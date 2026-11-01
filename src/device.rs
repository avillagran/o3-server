//! Connected Android targets only. No daemon or emulator is started by this module.
use crate::{
    p2::{invalid, str_field, Reply},
    store::State,
};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
pub struct Runtime {
    sessions: Mutex<Vec<Value>>,
}
pub fn available() -> bool {
    crate::executable::resolve(Path::new("adb")).is_some()
}
fn unavailable(reason: &str) -> Value {
    json!({"_tag":"DeviceToolUnavailableError","reason":reason,"message":reason})
}
pub async fn adb(args: &[&str]) -> Result<Vec<u8>, Value> {
    let exe = crate::executable::resolve(Path::new("adb"))
        .ok_or_else(|| unavailable("adb is not installed"))?;
    let out = crate::proc::run(
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
    .map_err(|_| unavailable("adb failed or timed out"))?;
    if !out.ok() || out.truncated {
        return Err(unavailable("adb command failed or exceeded output limit"));
    }
    Ok(out.stdout)
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
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
        || s.starts_with('-')
    {
        return Err(invalid("invalid deviceId"));
    }
    Ok(s)
}
pub async fn state(s: &State) -> Reply {
    let settings = crate::settings::read(s).map_err(invalid)?;
    let enabled = settings["deviceEnabled"].as_bool().unwrap_or(true);
    let mut devices = Vec::new();
    let mut status = "disabled";
    let mut detail = String::new();
    if enabled {
        match adb(&["devices", "-l"]).await {
            Ok(b) => {
                status = "ready";
                for line in String::from_utf8_lossy(&b).lines().skip(1).take(2048) {
                    let fields: Vec<_> = line.split_whitespace().collect();
                    if fields.len() < 2 || fields[1] != "device" {
                        continue;
                    }
                    let serial = fields[0];
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
    "onboardingCompleted":settings["deviceOnboardingCompleted"],"agentAccessEnabled":settings["deviceAgentAccessEnabled"].as_bool().unwrap_or(false),
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
    let serial = connected(p).await?;
    Ok(
        json!({"hostId":"local","deviceId":serial,"settings":{},"foregroundApp":null,"readAt":crate::store::iso(crate::store::now_ms())}),
    )
}
pub async fn call(s: &State, tag: &str, p: &Value) -> Reply {
    match tag {
        "device.list" => {
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
                ("enabled", "deviceEnabled"),
                ("agentAccessEnabled", "deviceAgentAccessEnabled"),
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
        "device.testHost" => Err(
            json!({"_tag":"DeviceHostUnavailableError","hostId":p["id"],"reason":"SSH device hosts are unsupported; no SSH process was started"}),
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
                return Err(unavailable(
                    "shutdown is unsupported; connected devices are left running",
                ));
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
            // adb shell joins argv on the remote shell: constrain every dynamic argument.
            let arg = |key| -> Result<&str, Value> {
                let v = str_field(p, key)?;
                if v.len() > 512
                    || !v
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._".contains(&c))
                {
                    return Err(invalid(key));
                }
                Ok(v)
            };
            match p["type"].as_str().unwrap_or("") {
                "launchApp" => {
                    adb(&["-s", serial, "shell", "monkey", "-p", arg("appId")?, "1"]).await?;
                }
                "terminateApp" => {
                    adb(&["-s", serial, "shell", "am", "force-stop", arg("appId")?]).await?;
                }
                _ => return Err(unavailable("this Android action is unsupported")),
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
            if let Ok(v) = state(&s).await {
                tx.send_replace(Some(v));
                s.streams.changed.send_modify(|v| *v += 1);
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
    pub fn next(&mut self) -> crate::terminal::Step {
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
