//! Async transport for an already owned local or SSH-forwarded native device hub.
//!
//! Construction does not start a daemon, invoke a shell, or discover host credentials.
use crate::device_native_hub::{self as hub, CommandResult, Device, Discovery, Platform, Request};
use serde_json::{json, Value};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// The host owner supplies its existing executable and platform resolution.
pub trait HostTools: Send + Sync {
    fn platform_available(&self, platform: Platform) -> HostFuture<'_, bool>;
    fn run<'a>(&'a self, command: &'a str, args: &'a [String]) -> HostFuture<'a, CommandResult>;
}
#[derive(Clone, Copy)]
struct Deadlines {
    list: Duration,
    boot: Duration,
    action: Duration,
    screenshot: Duration,
}
impl Default for Deadlines {
    fn default() -> Self {
        Self {
            list: Duration::from_secs(40),
            boot: Duration::from_secs(180),
            action: Duration::from_secs(15),
            screenshot: Duration::from_secs(20),
        }
    }
}
#[derive(Clone)]
pub struct HubClient {
    host_id: String,
    origin: String,
    http: reqwest::Client,
    tools: Arc<dyn HostTools>,
    deadlines: Deadlines,
}
fn failure(operation: &str, reason: &str, cause: impl ToString) -> Value {
    let mut error = hub::operation_error(operation, reason);
    error["cause"] = json!(cause.to_string());
    error
}
impl HubClient {
    /// Only loopback hub origins are accepted. SSH hosts must first own a local forward.
    pub fn new(host_id: String, origin: &str, tools: Arc<dyn HostTools>) -> Result<Self, String> {
        let url = reqwest::Url::parse(origin).map_err(|error| error.to_string())?;
        let loopback = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if url.scheme() != "http"
            || !loopback
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(
                "Device hubs require a loopback HTTP origin without credentials or a path".into(),
            );
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            host_id,
            origin: origin.trim_end_matches('/').into(),
            http,
            tools,
            deadlines: Deadlines::default(),
        })
    }
    async fn json(
        &self,
        plan: Request,
        operation: &str,
        deadline: Duration,
    ) -> Result<Value, Value> {
        let work = async {
            let mut request = self.http.request(
                reqwest::Method::from_bytes(plan.method.as_bytes()).expect("static HTTP method"),
                plan.url,
            );
            if let Some(body) = plan.body {
                request = request.json(&body);
            }
            let response = request
                .send()
                .await
                .map_err(|error| failure(operation, "request_failed", error))?;
            let response = response
                .error_for_status()
                .map_err(|error| failure(operation, "request_failed", error))?;
            // Redirects are rejected rather than allowing a local hub to send requests elsewhere.
            if !response.status().is_success() {
                return Err(failure(operation, "request_failed", response.status()));
            }
            response
                .json::<Value>()
                .await
                .map_err(|error| failure(operation, "request_failed", error))
        };
        tokio::time::timeout(deadline, work)
            .await
            .map_err(|_| failure(operation, "request_failed", "Device hub request timed out"))?
    }
    async fn listing(&self) -> Result<Discovery, Value> {
        // Keep the timeout outside decoding: only the deadline yields an incomplete snapshot.
        let work = async {
            let body = self
                .json(hub::list_request(&self.origin), "list", Duration::MAX)
                .await?;
            hub::parse_list(body).map_err(|mut error| {
                error["cause"] = json!("Invalid device list response");
                error
            })
        };
        let parsed = match tokio::time::timeout(self.deadlines.list, work).await {
            Ok(result) => result?,
            Err(_) => {
                let ios = self.tools.platform_available(Platform::Ios).await;
                let android = self.tools.platform_available(Platform::Android).await;
                return Ok(hub::listing_timeout(ios, android));
            }
        };
        let android = self.tools.platform_available(Platform::Android).await;
        let avds = if android {
            self.tools.run("emulator", &["-list-avds".into()]).await
        } else {
            CommandResult {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }
        };
        Ok(hub::discovery(parsed, android, &avds))
    }
    pub async fn list(&self) -> Result<Discovery, Value> {
        self.listing().await
    }
    async fn action(
        &self,
        plan: Request,
        operation: &str,
        deadline: Duration,
    ) -> Result<hub::HubAction, Value> {
        let body = self.json(plan, operation, deadline).await?;
        hub::parse_action(body, operation).map_err(|mut error| {
            error["cause"] = json!("Invalid device action response");
            error
        })
    }
    pub async fn attach(&self, device: &Device) -> Result<(), Value> {
        if let Some(plan) = hub::attach_request(&self.origin, device) {
            // A decoded ok:false is deliberately ignored by the native adapter here.
            self.action(plan, "attach stream", self.deadlines.boot)
                .await?;
        }
        Ok(())
    }
    pub async fn boot(&self, device: &Device) -> Result<String, Value> {
        let result = self
            .action(
                hub::boot_request(&self.origin, device),
                "boot",
                self.deadlines.boot,
            )
            .await?;
        let id = hub::boot_result(&self.host_id, device, result)?;
        self.attach(device).await?;
        Ok(id)
    }
    pub async fn shutdown(&self, device: &Device) -> Result<(), Value> {
        let result = self
            .action(
                hub::shutdown_request(&self.origin, device),
                "shutdown",
                self.deadlines.action,
            )
            .await;
        let result = result.and_then(|result| {
            if result.ok {
                Ok(())
            } else {
                let mut error = hub::operation_error("shutdown", "hub_rejected");
                error["cause"] = serde_json::to_value(result).expect("serializable hub action");
                Err(error)
            }
        });
        if let Err(error) = result {
            if device.platform == Platform::Ios
                && hub::shutdown_confirmed_off(&self.listing().await?, &device.id)
            {
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }
    pub async fn screenshot(&self, device: &Device) -> Result<Vec<u8>, Value> {
        let plan = hub::screenshot_request(&self.origin, device);
        let work = async {
            let response = self
                .http
                .post(plan.url)
                .send()
                .await
                .map_err(|error| failure("screenshot", "request_failed", error))?;
            if !response.status().is_success() {
                return Err(failure("screenshot", "hub_rejected", response.status()));
            }
            response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|error| failure("screenshot", "request_failed", error))
        };
        tokio::time::timeout(self.deadlines.screenshot, work)
            .await
            .map_err(|_| {
                failure(
                    "screenshot",
                    "request_failed",
                    "Device hub request timed out",
                )
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Tools {
        input: Value,
        runs: Mutex<Vec<Value>>,
    }
    impl HostTools for Tools {
        fn platform_available(&self, platform: Platform) -> HostFuture<'_, bool> {
            Box::pin(async move { self.input["available"][platform.label()] == true })
        }
        fn run<'a>(
            &'a self,
            command: &'a str,
            args: &'a [String],
        ) -> HostFuture<'a, CommandResult> {
            Box::pin(async move {
                self.runs
                    .lock()
                    .unwrap()
                    .push(json!({"command":command,"args":args}));
                serde_json::from_value(
                    self.input
                        .get("avds")
                        .cloned()
                        .unwrap_or_else(|| json!({"code":0,"stdout":"","stderr":""})),
                )
                .unwrap()
            })
        }
    }
    async fn evaluate(input: &Value) -> Value {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let source = input.clone();
        let server = tokio::spawn(async move {
            let mut position = 0;
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (head_end, length) = loop {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]);
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while bytes.len() < head_end + length {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    if count == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let head = String::from_utf8_lossy(&bytes[..head_end]);
                let mut line = head.lines().next().unwrap().split_whitespace();
                let method = line.next().unwrap();
                let path = line.next().unwrap();
                let body = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[head_end..head_end + length]).unwrap()
                };
                recorded.lock().unwrap().push(
                    json!({"method":method,"url":format!("http://device.test{path}"),"body":body}),
                );
                if source["timeout"] == true {
                    std::future::pending::<()>().await;
                }
                if source["transportFailure"] == true {
                    continue;
                }
                let responses = source["responses"].as_array().unwrap();
                let response = responses
                    .get(position)
                    .or_else(|| responses.last())
                    .unwrap();
                position += 1;
                let body = if let Some(binary) = response["binary"].as_array() {
                    binary
                        .iter()
                        .map(|byte| byte.as_u64().unwrap() as u8)
                        .collect::<Vec<_>>()
                } else {
                    serde_json::to_vec(&response["body"]).unwrap()
                };
                let status = response["status"].as_u64().unwrap_or(200);
                let header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",body.len());
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
        });
        let tools = Arc::new(Tools {
            input: input.clone(),
            runs: Mutex::new(Vec::new()),
        });
        let mut client = HubClient::new("local".into(), &origin, tools.clone()).unwrap();
        if input["timeout"] == true {
            let short = Duration::from_millis(30);
            client.deadlines = Deadlines {
                list: short,
                boot: short,
                action: short,
                screenshot: short,
            };
        }
        let device = input
            .get("device")
            .map(|value| serde_json::from_value::<Device>(value.clone()).unwrap());
        let result: Result<Value, Value> = match input["operation"].as_str().unwrap() {
            "list" => client
                .list()
                .await
                .map(|value| serde_json::to_value(value).unwrap()),
            "boot" => client
                .boot(device.as_ref().unwrap())
                .await
                .map(|value| json!(value)),
            "attach" => client
                .attach(device.as_ref().unwrap())
                .await
                .map(|_| Value::Null),
            "shutdown" => client
                .shutdown(device.as_ref().unwrap())
                .await
                .map(|_| Value::Null),
            "screenshot" => client
                .screenshot(device.as_ref().unwrap())
                .await
                .map(|value| json!(value)),
            _ => unreachable!(),
        };
        server.abort();
        let requests = requests.lock().unwrap().clone();
        let runs = tools.runs.lock().unwrap().clone();
        match result {
            Ok(value) => json!({"requests":requests,"runs":runs,"value":value}),
            Err(mut error) => {
                // Node HTTP exceptions carry runtime-specific stacks; native fixture projects them out.
                if error["_tag"] != "DeviceBootError" {
                    error.as_object_mut().unwrap().remove("cause");
                }
                json!({"requests":requests,"runs":runs,"error":error})
            }
        }
    }
    #[tokio::test]
    async fn pinned_native_hub_over_real_http() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-hub-4288.json"
        ))
        .unwrap();
        for family in ["list", "boot", "screenshots"] {
            for (index, case) in fixture[family].as_array().unwrap().iter().enumerate() {
                assert_eq!(
                    evaluate(&case["input"]).await,
                    case["expected"],
                    "{family} case {index}: {}",
                    case["input"]
                );
            }
        }
    }
    #[test]
    fn only_owned_loopback_hub_origins() {
        crate::test_sandbox::assert_isolated();
        for origin in [
            "https://localhost:80",
            "http://example.com",
            "http://127.0.0.1/path",
            "http://user@127.0.0.1",
            "http://127.0.0.1/?token=x",
        ] {
            assert!(HubClient::new(
                "local".into(),
                origin,
                Arc::new(Tools {
                    input: Value::Null,
                    runs: Mutex::new(Vec::new())
                })
            )
            .is_err());
        }
    }
}
