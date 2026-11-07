//! Native owned hubs when a verified installation exists; connected ADB targets remain a fallback.
use crate::{
    p2::{invalid, str_field, Reply},
    store::State,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
pub struct Runtime {
    sessions: Mutex<Vec<Value>>,
    cached: Mutex<Option<Value>>,
    native: tokio::sync::Mutex<Option<Arc<crate::device_native_runtime::NativeRuntime>>>,
    ssh: tokio::sync::Mutex<BTreeMap<String, SshRegistration>>,
    revision: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    test_ssh_executable: Mutex<Option<PathBuf>>,
}
struct SshRegistration {
    config: crate::device_native_ssh::Config,
    runtime: Arc<crate::device_native_runtime::NativeRuntime>,
}
fn ssh_configs(settings: &Value) -> Result<Vec<crate::device_native_ssh::Config>, Value> {
    let mut configs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for value in settings["deviceHosts"].as_array().into_iter().flatten() {
        let config = crate::device_native_ssh::Config::decode(value)
            .map_err(|error| invalid(error.to_string()))?;
        if !seen.insert(config.id.clone()) {
            return Err(invalid("Duplicate SSH device host ID"));
        }
        configs.push(config);
    }
    Ok(configs)
}
fn ssh_cli(
    s: &State,
    config: &crate::device_native_ssh::Config,
) -> Result<Arc<crate::device_native_ssh_transport::Cli>, Value> {
    let program = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    #[cfg(test)]
    let override_executable = s
        .streams
        .p2
        .device
        .test_ssh_executable
        .lock()
        .unwrap()
        .clone();
    #[cfg(not(test))]
    let override_executable: Option<PathBuf> = None;
    let executable = override_executable
        .or_else(|| crate::executable::resolve(Path::new(program)))
        .ok_or_else(|| {
            crate::device_native_owner::host_error(&config.id, "probe", "SSH is not installed")
        })?;
    let environment_id = std::fs::read_to_string(s.dir.join("environment-id")).ok();
    let owner = crate::device_native_ssh::owner(
        environment_id.as_deref(),
        &s.dir.to_string_lossy(),
        &config.id,
    );
    let cli = Arc::new(
        crate::device_native_ssh_transport::Cli::new(
            executable,
            s.dir.clone(),
            config.clone(),
            owner,
        )
        .map_err(|error| crate::device_native_owner::host_error(&config.id, "probe", error))?,
    );
    Ok(cli)
}
async fn ssh_runtimes(
    s: &State,
    settings: &Value,
) -> Result<Vec<Arc<crate::device_native_runtime::NativeRuntime>>, Value> {
    let configs = ssh_configs(settings)?;
    let mut registry = s.streams.p2.device.ssh.lock().await;
    let removed = registry
        .iter()
        .filter(|(_, entry)| !configs.iter().any(|config| config == &entry.config))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in removed {
        let entry = registry.get(&id).expect("retained registered owner");
        entry.runtime.shutdown().await?;
        registry.remove(&id);
    }
    for config in &configs {
        if registry.contains_key(&config.id) {
            continue;
        }
        let cli = ssh_cli(s, config)?;
        let summary = json!({"id":config.id,"kind":"ssh","label":config.label,"platforms":[],"hubInstalled":false,"agentDeviceInstalled":false});
        let runtime = crate::device_native_runtime::NativeRuntime::ssh(
            cli,
            summary,
            s.streams.changed.clone(),
        );
        registry.insert(
            config.id.clone(),
            SshRegistration {
                config: config.clone(),
                runtime,
            },
        );
    }
    Ok(configs
        .iter()
        .filter_map(|config| registry.get(&config.id).map(|entry| entry.runtime.clone()))
        .collect())
}
async fn selected_runtime(
    s: &State,
    p: &Value,
) -> Result<Option<Arc<crate::device_native_runtime::NativeRuntime>>, Value> {
    let host = p["hostId"].as_str().unwrap_or("local");
    if host == "local" {
        return native_runtime(s).await;
    }
    let settings = crate::settings::read(s).map_err(invalid)?;
    ssh_runtimes(s, &settings).await?;
    s.streams.p2.device.ssh.lock().await.get(host).map(|entry|Some(entry.runtime.clone())).ok_or_else(||json!({"_tag":"DeviceHostUnavailableError","hostId":host,"reason":"SSH device host is not configured","message":format!("Device host {host} is unavailable: SSH device host is not configured")}))
}
/// An invalid or incomplete installation is reported, never executed or silently replaced.
async fn native_runtime(
    s: &State,
) -> Result<Option<Arc<crate::device_native_runtime::NativeRuntime>>, Value> {
    let mut slot = s.streams.p2.device.native.lock().await;
    if let Some(runtime) = &*slot {
        return Ok(Some(runtime.clone()));
    }
    let root = s.asset_profile_root.join("tools/expo-device-hub/0.19.1");
    match std::fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(crate::device_native_owner::host_error(
                "local",
                "checking device support",
                error,
            ))
        }
        Ok(_) => {}
    }
    let node=crate::executable::resolve(Path::new("node")).ok_or_else(||json!({"_tag":"NodeRuntimeUnavailableError","feature":"Local device support","message":"Local device support requires Node.js. Install Node.js and make sure node is on PATH, then retry."}))?;
    let tools = Arc::new(crate::device_native_platform::LocalTools::discover(
        std::env::vars().collect(),
        s.dir.clone(),
    ));
    let platforms = [
        crate::device_native_hub::Platform::Ios,
        crate::device_native_hub::Platform::Android,
    ]
    .map(|platform| {
        let mut value =
            json!({"platform":platform,"available":tools.availability_reason(platform).is_none()});
        if let Some(reason) = tools.availability_reason(platform) {
            value["reason"] = json!(reason);
        }
        value
    });
    let driver = crate::device_native_local::LocalDriver::new(
        node,
        root,
        s.dir.clone(),
        tools.environment.clone(),
        tools.clone(),
        tools.platform.clone(),
    )
    .map_err(|error| {
        crate::device_native_owner::host_error("local", "checking device support", error)
    })?;
    let owner = crate::device_native_owner::LocalOwner::new(Arc::new(driver));
    let summary = json!({"id":"local","kind":"local","label":"This machine","platforms":platforms,"hubInstalled":true,"agentDeviceInstalled":false});
    let runtime = Arc::new(crate::device_native_runtime::NativeRuntime::new(
        owner,
        summary,
        s.streams.changed.clone(),
    ));
    *slot = Some(runtime.clone());
    Ok(Some(runtime))
}
/// Media routes use only the origin of a currently owned, verified hub.
pub async fn hub_origin(s: &State, host_id: &str) -> Result<Option<String>, Value> {
    let settings = crate::settings::read(s).map_err(invalid)?;
    let runtime = if host_id == "local" {
        let Some(runtime) = native_runtime(s).await? else {
            return Ok(None);
        };
        runtime
    } else {
        let registry = s.streams.p2.device.ssh.lock().await;
        let entry = registry.get(host_id).ok_or_else(|| {
            crate::device_native_owner::host_error(
                host_id,
                "accessing device support",
                "Device hub is not running",
            )
        })?;
        if !ssh_configs(&settings)?
            .iter()
            .any(|config| config == &entry.config)
        {
            return Err(crate::device_native_owner::host_error(
                host_id,
                "accessing device support",
                "Host configuration changed; retained owner has not stopped yet",
            ));
        }
        entry.runtime.clone()
    };
    runtime.configure(&settings).await?;
    runtime.origin().map(Some).ok_or_else(|| {
        crate::device_native_owner::host_error(
            host_id,
            "accessing device support",
            "Device hub is not running",
        )
    })
}
#[cfg(test)]
impl Runtime {
    pub(crate) fn register_ssh_executable_test(&self, executable: PathBuf) {
        *self.test_ssh_executable.lock().unwrap() = Some(executable);
    }
    pub(crate) async fn register_native_test(
        &self,
        runtime: Arc<crate::device_native_runtime::NativeRuntime>,
    ) {
        *self.native.lock().await = Some(runtime);
    }
}
pub fn available() -> bool {
    crate::executable::resolve(Path::new("adb")).is_some()
}
pub fn available_for(s: &State) -> bool {
    if available() {
        return true;
    }
    if let Ok(slot) = s.streams.p2.device.native.try_lock() {
        if let Some(runtime) = &*slot {
            if runtime.snapshot()["hosts"][0]["platforms"]
                .as_array()
                .is_some_and(|platforms| {
                    platforms
                        .iter()
                        .any(|platform| platform["available"] == true)
                })
            {
                return true;
            }
        }
    }
    if let Ok(registry) = s.streams.p2.device.ssh.try_lock() {
        if registry.values().any(|entry| {
            let view = entry.runtime.snapshot();
            view["hosts"][0]["hubInstalled"] == true
                && view["hosts"][0]["platforms"]
                    .as_array()
                    .is_some_and(|platforms| {
                        platforms
                            .iter()
                            .any(|platform| platform["available"] == true)
                    })
        }) {
            return true;
        }
    }
    let root = s.asset_profile_root.join("tools/expo-device-hub/0.19.1");
    if crate::executable::resolve(Path::new("node")).is_none()
        || crate::device_native_tools::InstalledTool::inspect(
            &root,
            crate::device_native_tools::Tool::Hub,
        )
        .is_err()
    {
        return false;
    }
    let tools = crate::device_native_platform::LocalTools::discover(
        std::env::vars().collect(),
        s.dir.clone(),
    );
    [
        crate::device_native_hub::Platform::Ios,
        crate::device_native_hub::Platform::Android,
    ]
    .into_iter()
    .any(|platform| tools.availability_reason(platform).is_none())
}
fn unavailable(reason: &str) -> Value {
    json!({"_tag":"DeviceToolUnavailableError","reason":reason,"message":reason})
}
async fn adb_output(args: &[&str]) -> Result<crate::proc::Out, Value> {
    let exe = crate::executable::resolve(Path::new("adb"))
        .ok_or_else(|| unavailable("adb is not installed"))?;
    crate::proc::run(
        exe.to_str().unwrap(),
        Path::new("/"),
        args,
        crate::proc::Opts {
            timeout: Duration::from_secs(10),
            cap: 8 * 1024 * 1024,
            allow_truncate: false,
        },
        &[],
    )
    .await
    .map_err(|_| unavailable("adb failed or timed out"))
}
pub async fn adb(args: &[&str]) -> Result<Vec<u8>, Value> {
    let out = adb_output(args).await?;
    if !out.ok() || out.truncated {
        return Err(unavailable("adb command failed or exceeded output limit"));
    }
    Ok(out.stdout)
}
fn action_unavailable(operation: &str) -> Value {
    json!({"_tag":"DeviceActionUnavailableError","operation":operation,"platform":"android","reason":"unsupported","message":format!("Device {operation} is not supported on android.")})
}
fn android_shell_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
async fn action_adb(serial: &str, args: &[&str], operation: &str) -> Result<Vec<u8>, Value> {
    let mut command = vec!["-s", serial];
    command.extend_from_slice(args);
    let out = adb_output(&command).await?;
    if !out.ok() || out.truncated {
        return Err(action_failure(operation, &out));
    }
    Ok(out.stdout)
}
fn action_failure(operation: &str, out: &crate::proc::Out) -> Value {
    let cause = json!({"code":out.code,"stdout":out.text(),"stderr":out.stderr});
    let message = if let Some(code) = out.code {
        format!("Device {operation} failed: The device command failed (exit code {code}).")
    } else {
        format!("Device {operation} failed: The device command failed.")
    };
    let mut error = json!({"_tag":"DeviceOperationError","operation":operation,"reason":"command_failed","cause":cause,"message":message});
    if let Some(code) = out.code {
        error["exitCode"] = json!(code);
    }
    error
}
async fn action_shell(serial: &str, args: &[&str], operation: &str) -> Result<Vec<u8>, Value> {
    // adb joins shell argv verbatim before the Android shell parses it.
    let words = args
        .iter()
        .map(|word| android_shell_word(word))
        .collect::<Vec<_>>();
    let mut command = vec!["shell"];
    command.extend(words.iter().map(String::as_str));
    action_adb(serial, &command, operation).await
}
fn js_number(value: &str) -> f64 {
    let value = crate::native_goal::trim(value);
    if value.is_empty() {
        return 0.0;
    }
    let prefix = value.get(..2).unwrap_or("");
    let radix = match prefix {
        "0x" | "0X" => 16,
        "0b" | "0B" => 2,
        "0o" | "0O" => 8,
        _ => return value.parse().unwrap_or(f64::NAN),
    };
    let digits = &value[2..];
    if digits.is_empty() {
        return f64::NAN;
    }
    digits
        .chars()
        .try_fold(0.0, |number, digit| {
            digit
                .to_digit(radix)
                .map(|digit| number * radix as f64 + digit as f64)
        })
        .unwrap_or(f64::NAN)
}
fn android_settings(
    night: Option<&str>,
    font: Option<&str>,
    animator: Option<&str>,
    wifi: Option<&str>,
    focus: Option<&str>,
) -> (Value, Value) {
    let mut settings = json!({});
    if night.is_some_and(|night| night.contains("yes")) {
        settings["appearance"] = json!("dark");
    } else if night.is_some_and(|night| night.contains("no")) {
        settings["appearance"] = json!("light");
    }
    if let Some(scale) = font
        .filter(|font| !font.is_empty() && *font != "null")
        .map(js_number)
        .filter(|scale| scale.is_finite())
    {
        settings["textSize"] = json!(if scale <= 0.9 {
            "small"
        } else if scale >= 1.25 {
            "extra-large"
        } else if scale >= 1.1 {
            "large"
        } else {
            "default"
        });
    }
    if let Some(animator) = animator.filter(|value| *value != "null") {
        settings["reduceMotion"] = json!(js_number(animator) == 0.0);
    }
    if let Some(wifi) = wifi.filter(|value| matches!(*value, "0" | "1")) {
        settings["networkEnabled"] = json!(wifi == "1");
    }
    static FOCUS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let focused = focus
        .and_then(|focus| {
            FOCUS
                .get_or_init(|| {
                    regex::Regex::new(
                        r"m(?:CurrentFocus|FocusedApp)=[A-Za-z0-9_]+\{[^ ]+ u[0-9]+ ([^/ ]+)/",
                    )
                    .unwrap()
                })
                .captures(focus)
        })
        .and_then(|captures| captures.get(1));
    (
        settings,
        focused
            .map(|id| json!({"id":id.as_str()}))
            .unwrap_or(Value::Null),
    )
}
fn landscape_native(size: &str) -> bool {
    static SIZE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    SIZE.get_or_init(|| regex::Regex::new(r"size: ([0-9]+)x([0-9]+)").unwrap())
        .captures_iter(size)
        .last()
        .is_some_and(|size| js_number(&size[1]) > js_number(&size[2]))
}
fn local(p: &Value) -> Result<(), Value> {
    if p.get("hostId")
        .and_then(Value::as_str)
        .is_some_and(|s| s != "local")
    {
        return Err(
            json!({"_tag":"DeviceHostUnavailableError","hostId":p["hostId"],"reason":"SSH and EAS hosts are unsupported; only local adb is available"}),
        );
    }
    Ok(())
}
pub fn id(p: &Value) -> Result<&str, Value> {
    local(p)?;
    let s = str_field(p, "deviceId")?;
    if s.len() > 256
        || !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:- ()".contains(&c))
        || s.starts_with('-')
    {
        return Err(invalid("invalid deviceId"));
    }
    Ok(s)
}
pub fn state(s: &State) -> crate::device_native_owner::HostFuture<'_, Reply> {
    Box::pin(async move {
        let view = Box::pin(state_inner(s)).await?;
        *s.streams.p2.device.cached.lock().unwrap() = Some(view.clone());
        Ok(view)
    })
}
fn append_view(state: &mut Value, view: &Value) {
    for field in ["hosts", "devices", "sessions", "bootingDevices"] {
        if let Some(values) = view[field].as_array() {
            if !state[field].is_array() {
                state[field] = json!([]);
            }
            state[field]
                .as_array_mut()
                .unwrap()
                .extend(values.iter().cloned());
        }
    }
    if let Some(statuses) = view["hostStatuses"].as_object() {
        for (id, status) in statuses {
            state["hostStatuses"][id] = status.clone();
        }
    }
}
async fn retained_state(s: &State) -> Option<Value> {
    let mut view = s.streams.p2.device.cached.lock().unwrap().clone()?;
    for field in ["hosts", "devices", "sessions", "bootingDevices"] {
        if let Some(values) = view[field].as_array_mut() {
            values.retain(|value| {
                if field == "hosts" {
                    value["id"] == "local"
                } else {
                    value["hostId"] == "local"
                }
            });
        }
    }
    if let Some(statuses) = view["hostStatuses"].as_object_mut() {
        statuses.retain(|id, _| id == "local");
    }
    for entry in s.streams.p2.device.ssh.lock().await.values() {
        append_view(&mut view, &entry.runtime.snapshot());
    }
    view["revision"] = json!(
        s.streams
            .p2
            .device
            .revision
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1
    );
    Some(view)
}
fn metadata_only_local(s: &State, settings: &Value) -> Value {
    json!({"hosts":[{"id":"local","kind":"local","label":"Local ADB","platforms":[{"platform":"android","available":available()},{"platform":"ios","available":false,"reason":"iOS requires native device tools"}],"hubInstalled":false,"agentDeviceInstalled":false}],"devices":[],"sessions":s.streams.p2.device.sessions.lock().unwrap().clone(),"hostStatus":if settings["enableDeviceSupport"]==false{"disabled"}else{"idle"},"hostStatuses":{},"onboardingCompleted":settings["deviceOnboardingCompleted"],"agentAccessEnabled":settings["enableAgentDeviceAccess"]==true,"hubBasePath":"/api/device-hub","supportsHostRetry":false,"supportsToolUpdate":false,"supportsEasHosts":false,"supportsSimlockHosts":false,"revision":0})
}
async fn inspect(s: &State) -> Reply {
    use futures_util::StreamExt;
    let settings = crate::settings::read(s).map_err(invalid)?;
    let configs = ssh_configs(&settings)?;
    let registered = s
        .streams
        .p2
        .device
        .ssh
        .lock()
        .await
        .values()
        .map(|entry| (entry.config.clone(), entry.runtime.clone()))
        .collect::<Vec<_>>();
    let mut view = s
        .streams
        .p2
        .device
        .cached
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| metadata_only_local(s, &settings));
    if let Some(local) = native_runtime(s).await? {
        let summary = local.snapshot()["hosts"][0].clone();
        if let Some(host) = view["hosts"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|host| host["id"] == "local")
        {
            *host = summary;
        }
    }
    let mut tasks: Vec<crate::device_native_owner::HostFuture<'_, (String, Value)>> = Vec::new();
    for config in &configs {
        let existing = registered
            .iter()
            .find(|(stored, _)| stored.id == config.id)
            .cloned();
        tasks.push(Box::pin(async move{
            let mut summary=json!({"id":config.id,"kind":"ssh","label":config.label,"platforms":[],"hubInstalled":false,"agentDeviceInstalled":false});
            let probe=if let Some((stored,runtime))=existing{
                if stored!=*config{summary=runtime.snapshot()["hosts"][0].clone();summary["toolInspectionError"]=json!("Could not confirm stopping the previous connection. Retry before changing this host.");return (config.id.clone(),summary);}
                let result=if let crate::device_native_runtime::OwnedHost::Ssh(owner)=&runtime.owner{owner.probe().await}else{Err(invalid("Expected SSH owner"))};runtime.update_owner_summary();summary=runtime.snapshot()["hosts"][0].clone();result
            }else{match ssh_cli(s,config){Ok(cli)=>cli.request(crate::device_native_ssh::RemoteMode::Probe,None).await,Err(error)=>Err(error)}};
            match probe{Ok(probe)=>{summary["platforms"]=probe["platforms"].clone();summary["hubInstalled"]=json!(probe["verifiedInstalled"]==true);summary.as_object_mut().unwrap().remove("toolInspectionError");if let Some(detail)=probe["detail"].as_str(){summary["toolInspectionError"]=json!(detail);}},Err(_)=>{summary["toolInspectionError"]=json!("Cannot check versions. Reconnect the host and check again. Installed tools have not been changed.");}}
            (config.id.clone(),summary)
        }));
    }
    for (id, summary) in futures_util::stream::iter(tasks)
        .buffered(4)
        .collect::<Vec<_>>()
        .await
    {
        let hosts = view["hosts"].as_array_mut().unwrap();
        if let Some(host) = hosts.iter_mut().find(|host| host["id"] == id) {
            *host = summary;
        } else {
            hosts.push(summary);
        }
    }
    view["onboardingCompleted"] = settings["deviceOnboardingCompleted"].clone();
    view["agentAccessEnabled"] = json!(settings["enableAgentDeviceAccess"] == true);
    view["revision"] = json!(
        s.streams
            .p2
            .device
            .revision
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1
    );
    Ok(view)
}
async fn state_inner(s: &State) -> Reply {
    let settings = crate::settings::read(s).map_err(invalid)?;
    let remote = ssh_runtimes(s, &settings).await?;
    use futures_util::StreamExt;
    let mut tasks: Vec<crate::device_native_owner::HostFuture<'_, Reply>> =
        vec![Box::pin(local_state(s))];
    let settings_ref = &settings;
    for runtime in remote {
        tasks.push(Box::pin(async move {
            runtime.configure(settings_ref).await?;
            Ok(match runtime.list().await {
                Ok(view) => view,
                Err(_) => runtime.snapshot(),
            })
        }));
    }
    let mut views = futures_util::stream::iter(tasks)
        .buffered(4)
        .collect::<Vec<_>>()
        .await
        .into_iter();
    let mut state = match views.next().expect("local host task") {
        Ok(state) => state,
        Err(error) => {
            json!({"hosts":[{"id":"local","kind":"local","label":"This machine","platforms":[],"hubInstalled":false,"agentDeviceInstalled":false}],"devices":[],"sessions":s.streams.p2.device.sessions.lock().unwrap().clone(),"hostStatus":"failed","hostStatusDetail":error["message"],"hostStatuses":{"local":{"status":"failed","detail":error["message"]}},"onboardingCompleted":settings["deviceOnboardingCompleted"],"agentAccessEnabled":settings["enableAgentDeviceAccess"]==true,"hubBasePath":"/api/device-hub","supportsHostRetry":false,"supportsToolUpdate":false,"supportsEasHosts":false,"supportsSimlockHosts":false})
        }
    };
    for view in views {
        let view = view?;
        append_view(&mut state, &view);
        state["supportsHostRetry"] = json!(true);
    }
    state["revision"] = json!(
        s.streams
            .p2
            .device
            .revision
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1
    );
    Ok(state)
}
async fn local_state(s: &State) -> Reply {
    let settings = crate::settings::read(s).map_err(invalid)?;
    if let Some(runtime) = native_runtime(s).await? {
        runtime.configure(&settings).await?;
        return match runtime.list().await {
            Ok(state) => Ok(state),
            Err(_) => Ok(runtime.snapshot()),
        };
    }
    let enabled = settings["enableDeviceSupport"].as_bool().unwrap_or(true);
    let mut devices = Vec::new();
    let mut status = "disabled";
    let mut detail = String::new();
    if enabled {
        match adb(&["devices", "-l"]).await {
            Ok(b) => {
                status = "ready";
                for line in String::from_utf8_lossy(&b).lines().skip(1).take(2048) {
                    let (serial, rest) = line
                        .split_once('\t')
                        .or_else(|| line.split_once(' '))
                        .unwrap_or(("", ""));
                    let fields: Vec<_> = rest.split_whitespace().collect();
                    if serial.is_empty()
                        || id(&json!({"deviceId":serial})).is_err()
                        || fields.first() != Some(&"device")
                    {
                        continue;
                    }
                    let name = fields
                        .iter()
                        .find_map(|s| s.strip_prefix("model:"))
                        .unwrap_or(serial);
                    devices.push(json!({"hostId":"local","id":serial,"name":name,"platform":"android","version":"unknown","booted":true,"physical":!serial.starts_with("emulator-")}));
                }
            }
            Err(_) => {
                status = "failed";
                detail = "adb unavailable or discovery failed".into();
            }
        }
    }
    Ok(
        json!({"hosts":[{"id":"local","kind":"local","label":"Local ADB","platforms":[{"platform":"android","available":available()},{"platform":"ios","available":false,"reason":"iOS is unsupported"}],"hubInstalled":false,"agentDeviceInstalled":false}],
    "devices":devices,"sessions":s.streams.p2.device.sessions.lock().unwrap().clone(),
    "hostStatus":status,"hostStatusDetail":detail,"hostStatuses":{"local":{"status":status,"detail":detail}},
    "onboardingCompleted":settings["deviceOnboardingCompleted"],"agentAccessEnabled":settings["enableAgentDeviceAccess"].as_bool().unwrap_or(false),
    "hubBasePath":"/api/device-hub","revision":crate::store::now_ms(),"supportsHostRetry":false,"supportsToolUpdate":false,"supportsEasHosts":false}),
    )
}
async fn connected(p: &Value) -> Result<&str, Value> {
    let serial = id(p)?;
    let status = adb(&["-s", serial, "get-state"]).await?;
    if status.as_slice() != b"device\n" && status.as_slice() != b"device\r\n" {
        return Err(unavailable("device is not connected and authorized"));
    }
    Ok(serial)
}
async fn detail(p: &Value) -> Reply {
    // Clients stop reading at 12 s, so slow optional settings are left out instead.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let serial = connected(p).await?;
    let read = |args: Vec<&'static str>, serial: &str| {
        let mut args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        args.splice(0..0, ["-s".to_owned(), serial.to_owned()]);
        async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            tokio::time::timeout_at(deadline, adb(&args))
                .await
                .ok()?
                .ok()
                .map(|v| crate::native_goal::trim(&String::from_utf8_lossy(&v)).to_owned())
        }
    };
    let (night, font, animator, wifi, focus) = tokio::join!(
        read(vec!["shell", "cmd", "uimode", "night"], serial),
        read(
            vec!["shell", "settings", "get", "system", "font_scale"],
            serial
        ),
        read(
            vec![
                "shell",
                "settings",
                "get",
                "global",
                "animator_duration_scale"
            ],
            serial
        ),
        read(
            vec!["shell", "settings", "get", "global", "wifi_on"],
            serial
        ),
        read(vec!["shell", "dumpsys", "window"], serial),
    );
    let (settings, foreground) = android_settings(
        night.as_deref(),
        font.as_deref(),
        animator.as_deref(),
        wifi.as_deref(),
        focus.as_deref(),
    );
    Ok(
        json!({"hostId":"local","deviceId":serial,"settings":settings,"foregroundApp":foreground,"readAt":crate::store::iso(crate::store::now_ms())}),
    )
}
pub fn call<'a>(
    s: &'a State,
    tag: &'a str,
    p: &'a Value,
) -> crate::device_native_owner::HostFuture<'a, Reply> {
    Box::pin(call_budget(s, tag, p))
}
async fn call_budget(s: &State, tag: &str, p: &Value) -> Reply {
    // Native boot has a three-minute hub deadline; do not apply the ADB nine-second budget.
    if matches!(tag, "device.open" | "device.close") {
        if let Some(runtime) = selected_runtime(s, p).await? {
            let settings = crate::settings::read(s).map_err(invalid)?;
            runtime.configure(&settings).await?;
            if tag == "device.open" && settings["enableDeviceSupport"] == false {
                return Err(unavailable("devices are disabled"));
            }
            return if tag == "device.open" {
                runtime.open(p).await
            } else {
                runtime.close(p).await
            };
        }
    }
    if matches!(tag, "device.action" | "device.open" | "device.close") {
        tokio::time::timeout(Duration::from_secs(9), Box::pin(call_inner(s, tag, p)))
            .await
            .unwrap_or_else(|_| Err(unavailable("device operation exceeded its time budget")))
    } else {
        Box::pin(call_inner(s, tag, p)).await
    }
}
async fn call_inner(s: &State, tag: &str, p: &Value) -> Reply {
    if matches!(tag, "device.open" | "device.action" | "device.detail") {
        let settings = crate::settings::read(s).map_err(invalid)?;
        if settings["enableDeviceSupport"] == false {
            return Err(unavailable("devices are disabled"));
        }
    }
    match tag {
        "device.list" => {
            if p["inspectOnly"] == true && p.get("updateTool").is_none() {
                return Box::pin(inspect(s)).await;
            }
            if p.get("updateTool").is_none() && p.get("retryHostId").is_some() {
                if let Some(runtime) =
                    selected_runtime(s, &json!({"hostId":p["retryHostId"]})).await?
                {
                    let settings = crate::settings::read(s).map_err(invalid)?;
                    runtime.configure(&settings).await?;
                    let _ = runtime.list().await;
                    return state(s).await;
                }
            }
            if p.get("updateTool").is_some() || p.get("retryHostId").is_some() {
                return Err(unavailable(
                    "tool installation and host retry are unsupported",
                ));
            }
            state(s).await
        }
        "device.configure" => {
            let mut patch = json!({});
            for (a, b) in [
                ("enabled", "enableDeviceSupport"),
                ("agentAccessEnabled", "enableAgentDeviceAccess"),
                ("onboardingCompleted", "deviceOnboardingCompleted"),
            ] {
                if let Some(v) = p.get(a) {
                    if !v.is_boolean() {
                        return Err(invalid(a));
                    }
                    patch[b] = v.clone();
                }
            }
            crate::settings::update(s, &json!({"patch":patch})).map_err(invalid)?;
            state(s).await
        }
        "device.testHost" => {
            let config = crate::device_native_ssh::Config::decode(p)
                .map_err(|error| invalid(error.to_string()))?;
            let registered = s
                .streams
                .p2
                .device
                .ssh
                .lock()
                .await
                .get(&config.id)
                .filter(|entry| entry.config == config)
                .map(|entry| entry.runtime.clone());
            let probe = if let Some(runtime) = registered {
                if let crate::device_native_runtime::OwnedHost::Ssh(owner) = &runtime.owner {
                    let probe = owner.probe().await?;
                    runtime.update_owner_summary();
                    probe
                } else {
                    return Err(invalid("Expected registered SSH owner"));
                }
            } else {
                ssh_cli(s, &config)?
                    .request(crate::device_native_ssh::RemoteMode::Probe, None)
                    .await?
            };
            let mut summary = json!({"id":config.id,"kind":"ssh","label":config.label,"platforms":probe["platforms"],"hubInstalled":probe["verifiedInstalled"]==true,"agentDeviceInstalled":false});
            if let Some(detail) = probe["detail"].as_str() {
                summary["toolInspectionError"] = json!(detail);
            }
            Ok(summary)
        }
        "device.simlockIdentity" => Err(
            json!({"_tag":"DeviceSimlockIdentityError","reason":"Simlock device gateways are unsupported by o3-server; no token was sent"}),
        ),
        "device.easAccount" => Err(
            json!({"_tag":"DeviceEasAccountError","reason":"EAS integration is unsupported; no credentials were stored"}),
        ),
        "device.open" => {
            if p["platform"] != "android" {
                return Err(unavailable(
                    "only connected Android devices are supported; no boot is performed",
                ));
            }
            let serial = connected(p).await?;
            let thread = str_field(p, "threadId")?;
            let session = json!({"threadId":thread,"hostId":"local","deviceId":serial,"platform":"android","openedAt":crate::store::iso(crate::store::now_ms())});
            let mut sessions = s.streams.p2.device.sessions.lock().unwrap();
            sessions.retain(|v| !(v["threadId"] == thread && v["deviceId"] == serial));
            if sessions.len() >= 4096 {
                return Err(invalid("session limit reached"));
            }
            sessions.push(session.clone());
            Ok(session)
        }
        "device.close" => {
            local(p)?;
            if p["shutdown"] == true {
                let serial = connected(p).await?;
                if serial.starts_with("emulator-") {
                    adb(&["-s", serial, "emu", "kill"]).await?;
                } else {
                    adb(&["-s", serial, "shell", "reboot", "-p"]).await?;
                }
            }
            let thread = str_field(p, "threadId")?;
            s.streams.p2.device.sessions.lock().unwrap().retain(|v| {
                !(v["threadId"] == thread
                    && p.get("deviceId").is_none_or(|id| v["deviceId"] == *id))
            });
            Ok(Value::Null)
        }
        "device.detail" => detail(p).await,
        "device.action" => {
            let serial = connected(p).await?;
            match p["type"].as_str().unwrap_or("") {
                "setOrientation" => {
                    let mut rotation = match p["value"].as_str() {
                        Some("portrait") => 0,
                        Some("landscape_left") => 1,
                        Some("portrait_upside_down") => 2,
                        Some("landscape_right") => 3,
                        _ => return Err(invalid("orientation")),
                    };
                    let size = action_shell(serial, &["wm", "size"], "orientation")
                        .await
                        .ok();
                    let landscape = size
                        .as_ref()
                        .is_some_and(|size| landscape_native(&String::from_utf8_lossy(size)));
                    if landscape {
                        rotation = (rotation + 3) % 4;
                    }
                    if serial.starts_with("emulator-") {
                        action_shell(
                            serial,
                            &["settings", "put", "system", "accelerometer_rotation", "1"],
                            "orientation",
                        )
                        .await?;
                        action_shell(
                            serial,
                            &["cmd", "window", "user-rotation", "free"],
                            "orientation",
                        )
                        .await?;
                        action_adb(
                            serial,
                            &[
                                "emu",
                                "sensor",
                                "set",
                                "acceleration",
                                ["0:9.81:0", "9.81:0:0", "0:-9.81:0", "-9.81:0:0"][rotation],
                            ],
                            "orientation",
                        )
                        .await?;
                    } else {
                        action_shell(
                            serial,
                            &[
                                "cmd",
                                "window",
                                "user-rotation",
                                "lock",
                                &rotation.to_string(),
                            ],
                            "orientation",
                        )
                        .await?;
                    }
                }
                "setAppearance" => {
                    let value = match p["value"].as_str() {
                        Some("dark") => "yes",
                        Some("light") => "no",
                        _ => return Err(invalid("appearance")),
                    };
                    action_shell(serial, &["cmd", "uimode", "night", value], "appearance").await?;
                }
                "setTextSize" => {
                    let value = match p["value"].as_str() {
                        Some("small") => "0.85",
                        Some("default") => "1.0",
                        Some("large") => "1.15",
                        Some("extra-large") => "1.3",
                        _ => return Err(invalid("text size")),
                    };
                    action_shell(
                        serial,
                        &["settings", "put", "system", "font_scale", value],
                        "text size",
                    )
                    .await?;
                }
                "setToggle" => {
                    let enabled = p["value"]
                        .as_bool()
                        .ok_or_else(|| invalid("toggle value"))?;
                    match p["setting"].as_str() {
                        Some("networkEnabled") => {
                            for service in ["wifi", "data"] {
                                action_shell(
                                    serial,
                                    &["svc", service, if enabled { "enable" } else { "disable" }],
                                    "network",
                                )
                                .await?;
                            }
                        }
                        Some("reduceMotion") => {
                            for key in [
                                "animator_duration_scale",
                                "transition_animation_scale",
                                "window_animation_scale",
                            ] {
                                action_shell(
                                    serial,
                                    &[
                                        "settings",
                                        "put",
                                        "global",
                                        key,
                                        if enabled { "0" } else { "1" },
                                    ],
                                    "reduce motion",
                                )
                                .await?;
                            }
                        }
                        _ => return Err(action_unavailable("setToggle")),
                    }
                }
                "setLocation" => {
                    let number = |key, minimum, maximum| {
                        p[key]
                            .as_f64()
                            .filter(|number| {
                                number.is_finite() && *number >= minimum && *number <= maximum
                            })
                            .map(|number| {
                                if number == 0.0 {
                                    "0".to_owned()
                                } else {
                                    ryu_js::Buffer::new().format(number).to_owned()
                                }
                            })
                            .ok_or_else(|| invalid(key))
                    };
                    let latitude = number("latitude", -90.0, 90.0)?;
                    let longitude = number("longitude", -180.0, 180.0)?;
                    action_adb(
                        serial,
                        &["emu", "geo", "fix", &longitude, &latitude],
                        "location",
                    )
                    .await?;
                }
                "clearLocation" => {}
                "setPermission" => {
                    let permissions: &[&str] = match p["permission"].as_str() {
                        Some("camera") => &["android.permission.CAMERA"],
                        Some("microphone") => &["android.permission.RECORD_AUDIO"],
                        Some("photos") => &[
                            "android.permission.READ_MEDIA_IMAGES",
                            "android.permission.READ_EXTERNAL_STORAGE",
                        ],
                        Some("contacts") => &[
                            "android.permission.READ_CONTACTS",
                            "android.permission.WRITE_CONTACTS",
                        ],
                        Some("calendar") => &[
                            "android.permission.READ_CALENDAR",
                            "android.permission.WRITE_CALENDAR",
                        ],
                        Some("location") => &[
                            "android.permission.ACCESS_FINE_LOCATION",
                            "android.permission.ACCESS_COARSE_LOCATION",
                        ],
                        Some("notifications") => &["android.permission.POST_NOTIFICATIONS"],
                        Some("motion") => &["android.permission.ACTIVITY_RECOGNITION"],
                        _ => return Err(action_unavailable("permission")),
                    };
                    let verb = match p["decision"].as_str() {
                        Some("grant") => "grant",
                        Some("revoke" | "reset") => "revoke",
                        _ => return Err(invalid("permission decision")),
                    };
                    let app = str_field(p, "appId")?;
                    let mut first_failure = None;
                    let mut succeeded = false;
                    for permission in permissions {
                        match action_shell(serial, &["pm", verb, app, permission], "permission")
                            .await
                        {
                            Ok(_) => succeeded = true,
                            Err(error) => {
                                if first_failure.is_none() {
                                    first_failure = Some(error);
                                }
                            }
                        }
                    }
                    if !succeeded {
                        return Err(first_failure.unwrap());
                    }
                }
                "openUrl" => {
                    action_shell(
                        serial,
                        &[
                            "am",
                            "start",
                            "-a",
                            "android.intent.action.VIEW",
                            "-d",
                            str_field(p, "url")?,
                        ],
                        "open url",
                    )
                    .await?;
                }
                "launchApp" => {
                    action_shell(
                        serial,
                        &[
                            "monkey",
                            "-p",
                            str_field(p, "appId")?,
                            "-c",
                            "android.intent.category.LAUNCHER",
                            "1",
                        ],
                        "launch",
                    )
                    .await?;
                }
                "terminateApp" => {
                    action_shell(
                        serial,
                        &["am", "force-stop", str_field(p, "appId")?],
                        "terminate",
                    )
                    .await?;
                }
                "setLiquidGlass" | "setColorFilter" | "sendPush" | "shake" => {
                    return Err(action_unavailable(p["type"].as_str().unwrap()))
                }
                _ => return Err(invalid("unknown device action")),
            }
            detail(p).await
        }
        _ => Err(unavailable("unsupported device operation")),
    }
}
pub struct Cursor {
    task: tokio::task::JoinHandle<()>,
    rx: tokio::sync::watch::Receiver<Option<Value>>,
    waiting: bool,
}
impl Drop for Cursor {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub fn subscribe(s: &Arc<State>) -> Cursor {
    let (tx, rx) = tokio::sync::watch::channel(None);
    let s = s.clone();
    let task = tokio::spawn(async move {
        loop {
            let view = match state(&s).await {
                Ok(view) => Some(view),
                Err(_) => retained_state(&s).await,
            };
            if let Some(view) = view {
                tx.send_replace(Some(view));
                s.streams.changed.send_modify(|version| *version += 1);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    Cursor {
        task,
        rx,
        waiting: false,
    }
}
impl Cursor {
    pub fn poll(&mut self) -> crate::terminal::Step {
        if !self.waiting && self.rx.has_changed().unwrap_or(false) {
            let v = self.rx.borrow_and_update().clone();
            if let Some(v) = v {
                self.waiting = true;
                return crate::terminal::Step::Values(vec![v]);
            }
        }
        crate::terminal::Step::Idle
    }
    pub fn ack(&mut self) {
        self.waiting = false;
    }
    pub fn is_full(&self) -> bool {
        self.waiting
    }
}

// All remote-shell arguments below are generated decimal numbers or fixed words.
pub async fn input(serial: &str, v: &Value) -> Reply {
    if v["type"] == "touch" {
        if !matches!(v["action"].as_str(), Some("down" | "move" | "up")) {
            return Err(invalid("touch action"));
        }
        let unit = |k| {
            v[k].as_f64()
                .filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
                .ok_or_else(|| invalid("coordinate"))
        };
        let (x, y) = (unit("x")?, unit("y")?);
        if v["action"] != "up" {
            return Ok(Value::Null);
        }
        let size = adb(&["-s", serial, "shell", "wm", "size"]).await?;
        let text = String::from_utf8_lossy(&size);
        let dims = text
            .lines()
            .last()
            .and_then(|l| l.split_whitespace().last())
            .and_then(|d| d.split_once('x'))
            .ok_or_else(|| unavailable("display size unavailable"))?;
        let w = dims.0.parse::<u32>().map_err(|_| invalid("width"))?;
        let h = dims.1.parse::<u32>().map_err(|_| invalid("height"))?;
        let (x, y) = (
            (x * w.saturating_sub(1) as f64) as u32,
            (y * h.saturating_sub(1) as f64) as u32,
        );
        adb(&[
            "-s",
            serial,
            "shell",
            "input",
            "tap",
            &x.to_string(),
            &y.to_string(),
        ])
        .await?;
        return Ok(Value::Null);
    }
    let key = match v["type"].as_str().unwrap_or("") {
        "reset-video" => {
            return Err(unavailable(
                "H264 video is unavailable; use the screenshot endpoint",
            ))
        }
        "key" => v["keycode"]
            .as_u64()
            .filter(|k| *k > 0 && *k < 1000)
            .ok_or_else(|| invalid("keycode"))?
            .to_string(),
        _ => return Err(unavailable("only Android key input is currently supported")),
    };
    adb(&["-s", serial, "shell", "input", "keyevent", &key]).await?;
    Ok(Value::Null)
}

/// One recorder owned by one viewer socket; caller kills and reaps it on disconnect.
pub(crate) fn recorder(serial: &str) -> Result<tokio::process::Child, std::io::Error> {
    let executable = crate::executable::resolve(Path::new("adb"))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "adb unavailable"))?;
    tokio::process::Command::new(executable)
        .args([
            "-s",
            serial,
            "exec-out",
            "screenrecord",
            "--output-format=h264",
            "--bit-rate",
            "2000000",
            "-",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

#[cfg(test)]
mod native_action_tests {
    use super::*;
    #[test]
    fn source_errors_keep_native_action_metadata_and_messages() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/device-actions.json")).unwrap();
        for case in fixture["actionCases"].as_array().unwrap() {
            let Some(expected) = case["expected"].get("error") else {
                continue;
            };
            let operation = expected["operation"].as_str().unwrap();
            let error = if expected["_tag"] == "DeviceActionUnavailableError" {
                action_unavailable(operation)
            } else {
                action_failure(
                    operation,
                    &crate::proc::Out {
                        code: Some(1),
                        stdout: vec![],
                        stderr: "synthetic command failure".into(),
                        truncated: false,
                    },
                )
            };
            for (key, value) in expected.as_object().unwrap() {
                assert_eq!(&error[key], value, "{}: {key}", case["label"]);
            }
        }
    }
}

#[cfg(test)]
#[path = "device_native_registry_tests.rs"]
mod device_native_registry_tests;
