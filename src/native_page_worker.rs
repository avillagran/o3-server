//! Inactive native Page process supervisor. Only a trusted owner can supply a
//! scoped grant; this module is not an MCP endpoint or a browser launcher.
pub mod cdp;
pub mod control;

use serde_json::{json, Value};
use std::{
    collections::BTreeMap, future::Future, path::PathBuf, pin::Pin, process::Stdio, sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, oneshot, watch},
    time::Instant,
};
const MAGIC: &[u8; 8] = b"O3PG0001";
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_PENDING: usize = 8;
const MAX_STDERR_BYTES: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    InvalidInstallation,
    SpawnFailed,
    Protocol,
    TooLarge,
    Busy,
    Deadline,
    Revoked,
    Exited,
    Closed,
    CleanupUnknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseReceipt {
    pub cause: Fault,
    pub worker_reaped: bool,
    pub owner_grant_closed: bool,
    pub graceful_worker: bool,
    pub native_cleanup_requests: Option<u64>,
    pub executed_recursive_removals: Option<u64>,
}
/// Implemented by the Rust tab owner. The endpoint must already be target-
/// scoped; caller arguments must never be used to construct this authority.
pub trait GrantOwner: Send + Sync + 'static {
    fn target_id(&self) -> &str;
    fn endpoint(&self) -> &str;
    fn revocation(&self) -> watch::Receiver<bool>;
    /// Synchronously fence this grant before any process cleanup awaits.
    fn revoke(&self);
    fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Fault>> + Send + '_>>;
}
/// Trusted sealed-installation inputs. Verify package manifests before building
/// this specification. No argument or environment inherits an MCP payload.
#[derive(Clone)]
pub struct WorkerSpec {
    pub node: PathBuf,
    pub entry: PathBuf,
    pub private_home: PathBuf,
    pub configuration: Value,
    pub startup_deadline: Duration,
    pub close_deadline: Duration,
}
impl WorkerSpec {
    fn validate(&self) -> Result<(), Fault> {
        for p in [&self.node, &self.entry] {
            let m = std::fs::symlink_metadata(p).map_err(|_| Fault::InvalidInstallation)?;
            if !p.is_absolute() || !m.is_file() || m.file_type().is_symlink() {
                return Err(Fault::InvalidInstallation);
            }
        }
        let home = std::fs::symlink_metadata(&self.private_home)
            .map_err(|_| Fault::InvalidInstallation)?;
        use std::os::unix::fs::PermissionsExt;
        if !self.private_home.is_absolute()
            || !home.is_dir()
            || home.file_type().is_symlink()
            || home.permissions().mode() & 0o777 != 0o700
            || self.startup_deadline.is_zero()
            || self.startup_deadline > Duration::from_secs(30)
            || self.close_deadline.is_zero()
            || self.close_deadline > Duration::from_secs(30)
        {
            return Err(Fault::InvalidInstallation);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum OperationResult {
    Value(Value),
    NativeError(Value),
    /// Exact JavaScript JSON which cannot be represented by a Unicode-scalar
    /// serde Value. Never substitute a lossy string in an MCP response.
    JavaScriptValue(JavaScriptJson),
    JavaScriptError(JavaScriptJson),
}
#[derive(Clone, Debug)]
pub struct JavaScriptJson(Box<serde_json::value::RawValue>);
impl JavaScriptJson {
    pub fn json(&self) -> &str {
        self.0.get()
    }
    /// Validate one JSON value without a Unicode-scalar application conversion.
    /// The owner/transport must enforce its byte cap before creating this value.
    pub fn parse(raw: String) -> Result<Self, serde_json::Error> {
        serde_json::value::RawValue::from_string(raw).map(Self)
    }
}
impl PartialEq for JavaScriptJson {
    fn eq(&self, other: &Self) -> bool {
        self.json() == other.json()
    }
}
impl serde::Serialize for JavaScriptJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
struct Packet {
    envelope: Value,
    exact: Option<OperationResult>,
}
impl std::ops::Deref for Packet {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.envelope
    }
}
fn decode_packet(bytes: &[u8]) -> Result<Packet, Fault> {
    if let Ok(envelope) = serde_json::from_slice(bytes) {
        return Ok(Packet {
            envelope,
            exact: None,
        });
    }
    // Parse only the control envelope as scalar JSON. RawValue validates the
    // original JSON grammar while retaining every escaped UTF-16 payload unit.
    let mut fields: BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(bytes).map_err(|_| Fault::Protocol)?;
    let value = fields.remove("value");
    let error = fields.remove("error");
    let (field, payload, failed) = match (value, error) {
        (Some(value), None) => ("value", value, false),
        (None, Some(error)) if error.get().trim_start().starts_with('{') => ("error", error, true),
        _ => return Err(Fault::Protocol),
    };
    let mut envelope = serde_json::Map::new();
    for (key, raw) in fields {
        envelope.insert(
            key,
            serde_json::from_str(raw.get()).map_err(|_| Fault::Protocol)?,
        );
    }
    if envelope.get("kind") != Some(&json!("result")) || envelope.get("ok") != Some(&json!(!failed))
    {
        return Err(Fault::Protocol);
    }
    envelope.insert(field.into(), if failed { json!({}) } else { Value::Null });
    let exact = if failed {
        OperationResult::JavaScriptError(JavaScriptJson(payload))
    } else {
        OperationResult::JavaScriptValue(JavaScriptJson(payload))
    };
    Ok(Packet {
        envelope: Value::Object(envelope),
        exact: Some(exact),
    })
}
/// Native broker grace is response transport time, never page-operation time.
pub const HOST_RESPONSE_GRACE: Duration = Duration::from_millis(1_000);

struct Call {
    native_timeout: Duration,
    operation: String,
    input: Value,
    deadline: Instant,
    reply: oneshot::Sender<Result<OperationResult, Fault>>,
}
struct Pending {
    deadline: Instant,
    reply: oneshot::Sender<Result<OperationResult, Fault>>,
}
struct Handle {
    calls: mpsc::Sender<Call>,
    stop: watch::Sender<bool>,
    closed: watch::Receiver<Option<CloseReceipt>>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}
#[derive(Clone)]
pub struct Worker {
    handle: Arc<Handle>,
}
impl Worker {
    pub async fn spawn(spec: WorkerSpec, owner: Arc<dyn GrantOwner>) -> Result<Self, CloseReceipt> {
        if spec.validate().is_err() {
            return Err(close_without_child(&spec, &owner, Fault::InvalidInstallation).await);
        }
        let url = match url::Url::parse(owner.endpoint()) {
            Ok(url)
                if url.scheme() == "ws"
                    && url.host_str() == Some("127.0.0.1")
                    && url.port().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
                    && url.path().len() >= 32 =>
            {
                url
            }
            _ => return Err(close_without_child(&spec, &owner, Fault::Protocol).await),
        };
        if owner.target_id().is_empty() || *owner.revocation().borrow() {
            return Err(close_without_child(&spec, &owner, Fault::Revoked).await);
        }
        let mut command = Command::new(&spec.node);
        command
            .arg(&spec.entry)
            .current_dir(&spec.private_home)
            .env_clear()
            .env("HOME", &spec.private_home)
            .env("XDG_CONFIG_HOME", spec.private_home.join("config"))
            .env("XDG_CACHE_HOME", spec.private_home.join("cache"))
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => return Err(close_without_child(&spec, &owner, Fault::SpawnFailed).await),
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill().await;
            return Err(close_without_child(&spec, &owner, Fault::SpawnFailed).await);
        };
        let (calls, rx) = mpsc::channel(MAX_PENDING);
        let (stop, stop_rx) = watch::channel(false);
        let (closed_tx, closed) = watch::channel(None);
        let (ready_tx, ready) = oneshot::channel();
        let mut bytes = [0u8; 24];
        if getrandom::fill(&mut bytes).is_err() {
            let _ = child.kill().await;
            return Err(close_without_child(&spec, &owner, Fault::SpawnFailed).await);
        }
        let nonce = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let bootstrap = json!({"kind":"init","protocol":1,"nonce":nonce,"targetId":owner.target_id(),"endpoint":url.as_str(),"configuration":spec.configuration});
        tokio::spawn(run(Running {
            child,
            stdin,
            stdout,
            stderr,
            calls: rx,
            stop: stop_rx,
            closed: closed_tx,
            ready: ready_tx,
            spec,
            owner,
            bootstrap,
            nonce,
        }));
        match ready.await {
            Ok(Ok(())) => Ok(Self {
                handle: Arc::new(Handle {
                    calls,
                    stop,
                    closed,
                }),
            }),
            _ => {
                let mut closed = closed;
                loop {
                    if let Some(receipt) = closed.borrow().clone() {
                        return Err(receipt);
                    }
                    if closed.changed().await.is_err() {
                        return Err(CloseReceipt {
                            cause: Fault::Exited,
                            worker_reaped: false,
                            owner_grant_closed: false,
                            graceful_worker: false,
                            native_cleanup_requests: None,
                            executed_recursive_removals: None,
                        });
                    }
                }
            }
        }
    }
    /// Owner-only reference invalidation. It preserves this process, Page and
    /// keyboard; an MCP caller must not choose private protocol operations.
    pub async fn invalidate_refs(&self) -> Result<(), Fault> {
        match self
            .invoke("$owner.invalidateRefs", Value::Null, Duration::from_secs(5))
            .await?
        {
            OperationResult::Value(Value::Bool(true)) => Ok(()),
            _ => Err(Fault::Protocol),
        }
    }
    pub async fn invoke(
        &self,
        operation: &str,
        input: Value,
        timeout: Duration,
    ) -> Result<OperationResult, Fault> {
        self.invoke_budget(operation, input, timeout, Duration::ZERO)
            .await
    }
    /// Preserve the native operation timeout while allowing its original host
    /// response grace only when the broker updates the current tab.
    pub async fn invoke_native(
        &self,
        operation: &str,
        input: Value,
        native_timeout: Duration,
        update_current_tab: bool,
    ) -> Result<OperationResult, Fault> {
        if native_timeout.as_millis() == 0
            || !native_timeout.subsec_nanos().is_multiple_of(1_000_000)
        {
            return Err(Fault::Deadline);
        }
        self.invoke_budget(
            operation,
            input,
            native_timeout,
            if update_current_tab {
                HOST_RESPONSE_GRACE
            } else {
                Duration::ZERO
            },
        )
        .await
    }
    async fn invoke_budget(
        &self,
        operation: &str,
        input: Value,
        timeout: Duration,
        grace: Duration,
    ) -> Result<OperationResult, Fault> {
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(Fault::Deadline);
        }
        if *self.handle.stop.borrow() || self.handle.closed.borrow().is_some() {
            return Err(Fault::Closed);
        }
        let (reply, result) = oneshot::channel();
        self.handle
            .calls
            .try_send(Call {
                operation: operation.into(),
                input,
                native_timeout: timeout,
                deadline: Instant::now() + timeout + grace,
                reply,
            })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => Fault::Busy,
                mpsc::error::TrySendError::Closed(_) => Fault::Closed,
            })?;
        result.await.unwrap_or(Err(Fault::Exited))
    }
    /// The receipt confirms both the Node reap and independent owner grant close.
    /// An owner timeout/refusal remains false even if the child has exited.
    pub async fn close(&self) -> CloseReceipt {
        let _ = self.handle.stop.send(true);
        let mut closed = self.handle.closed.clone();
        loop {
            if let Some(value) = closed.borrow().clone() {
                return value;
            }
            if closed.changed().await.is_err() {
                return CloseReceipt {
                    cause: Fault::Exited,
                    worker_reaped: false,
                    owner_grant_closed: false,
                    graceful_worker: false,
                    native_cleanup_requests: None,
                    executed_recursive_removals: None,
                };
            }
        }
    }
}
async fn close_without_child(
    spec: &WorkerSpec,
    owner: &Arc<dyn GrantOwner>,
    cause: Fault,
) -> CloseReceipt {
    let limit = if spec.close_deadline.is_zero() {
        Duration::from_secs(1)
    } else {
        spec.close_deadline.min(Duration::from_secs(30))
    };
    owner.revoke();
    let done = matches!(tokio::time::timeout(limit, owner.close()).await, Ok(Ok(())));
    CloseReceipt {
        cause,
        worker_reaped: true,
        owner_grant_closed: done,
        graceful_worker: false,
        native_cleanup_requests: None,
        executed_recursive_removals: None,
    }
}
async fn read_frame<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Result<Packet, Fault> {
    let mut header = [0u8; 12];
    reader
        .read_exact(&mut header)
        .await
        .map_err(|_| Fault::Exited)?;
    if &header[..8] != MAGIC {
        return Err(Fault::Protocol);
    }
    let size = u32::from_le_bytes(header[8..].try_into().map_err(|_| Fault::Protocol)?) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(Fault::TooLarge);
    }
    let mut body = vec![0; size];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|_| Fault::Protocol)?;
    decode_packet(&body)
}
async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), Fault> {
    let body = serde_json::to_vec(value).map_err(|_| Fault::Protocol)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(Fault::TooLarge);
    }
    writer.write_all(MAGIC).await.map_err(|_| Fault::Exited)?;
    writer
        .write_all(&(body.len() as u32).to_le_bytes())
        .await
        .map_err(|_| Fault::Exited)?;
    writer.write_all(&body).await.map_err(|_| Fault::Exited)?;
    writer.flush().await.map_err(|_| Fault::Exited)
}
struct Running {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    calls: mpsc::Receiver<Call>,
    stop: watch::Receiver<bool>,
    closed: watch::Sender<Option<CloseReceipt>>,
    ready: oneshot::Sender<Result<(), Fault>>,
    spec: WorkerSpec,
    owner: Arc<dyn GrantOwner>,
    bootstrap: Value,
    nonce: String,
}
async fn run(running: Running) {
    let Running {
        mut child,
        mut stdin,
        mut stdout,
        mut stderr,
        mut calls,
        mut stop,
        closed,
        ready,
        spec,
        owner,
        bootstrap,
        nonce,
    } = running;
    let mut revoked = owner.revocation();
    let (frames, mut inbound) = mpsc::channel(8);
    let (writes, mut outbound) = mpsc::channel::<Value>(8);
    let (fault_tx, mut faults) = mpsc::channel(2);
    let reader = tokio::spawn(async move {
        loop {
            let packet = read_frame(&mut stdout).await;
            let ended = packet.is_err();
            if frames.send(packet).await.is_err() || ended {
                break;
            }
        }
    });
    let writer = tokio::spawn(async move {
        while let Some(packet) = outbound.recv().await {
            if let Err(error) = write_frame(&mut stdin, &packet).await {
                let _ = fault_tx.send(error).await;
                break;
            }
        }
    });
    let (stderr_fault, mut stderr_fail) = mpsc::channel(1);
    let stderr_task = tokio::spawn(async move {
        let mut total = 0;
        let mut bytes = [0; 4096];
        loop {
            match stderr.read(&mut bytes).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    total += n;
                    if total > MAX_STDERR_BYTES {
                        let _ = stderr_fault.send(Fault::TooLarge).await;
                        break;
                    }
                }
            }
        }
    });
    let initially_revoked = *revoked.borrow();
    if !initially_revoked {
        let _ = writes.send(bootstrap).await;
    }
    let handshake = if initially_revoked {
        Err(Fault::Revoked)
    } else {
        tokio::select! {packet=inbound.recv()=>match packet{Some(Ok(v))if v.envelope==json!({"kind":"ready","protocol":1,"nonce":nonce})=>Ok(()),Some(Err(e))=>Err(e),_=>Err(Fault::Protocol)},_=tokio::time::sleep(spec.startup_deadline)=>Err(Fault::Deadline),_=stop.changed()=>Err(Fault::Closed),_=revoked.changed()=>Err(Fault::Revoked),Some(error)=faults.recv()=>Err(error),Some(error)=stderr_fail.recv()=>Err(error),_=child.wait()=>Err(Fault::Exited)}
    };
    let mut pending = BTreeMap::<u64, Pending>::new();
    let mut next = 0u64;
    let cause = if let Err(error) = handshake {
        let _ = ready.send(Err(error));
        error
    } else {
        let _ = ready.send(Ok(()));
        loop {
            let deadline = pending
                .values()
                .map(|p| p.deadline)
                .min()
                .unwrap_or(Instant::now() + Duration::from_secs(86400));
            tokio::select! {
                    _=stop.changed()=>break Fault::Closed,
                    _=revoked.changed()=>break Fault::Revoked,
                    status=child.wait()=>{let _=status;break Fault::Exited;},
                    Some(fault)=faults.recv()=>break fault,
                    Some(fault)=stderr_fail.recv()=>break fault,
                    _=tokio::time::sleep_until(deadline),if !pending.is_empty()=>break Fault::Deadline,
                    call=calls.recv(),if pending.len()<MAX_PENDING=>{
                     let Some(call)=call else{break Fault::Closed;};if *revoked.borrow()||*stop.borrow(){let _=call.reply.send(Err(Fault::Revoked));break Fault::Revoked;}
            if call.reply.is_closed(){continue;}
            if call.deadline<=Instant::now(){let _=call.reply.send(Err(Fault::Deadline));continue;}
                     next+=1;if next>9_007_199_254_740_991{break Fault::Protocol;}
                     let packet=json!({"kind":"invoke","id":next,"nonce":nonce,"operation":call.operation,"input":call.input,"timeoutMs":call.native_timeout.as_millis().max(1)});
                     if serde_json::to_vec(&packet).map_or(true,|b|b.len()>MAX_FRAME_BYTES){let _=call.reply.send(Err(Fault::TooLarge));continue;}
                     pending.insert(next,Pending{deadline:call.deadline,reply:call.reply});if writes.try_send(packet).is_err(){break Fault::Busy;}
                    },
                    packet=inbound.recv()=>{
                     let mut packet=match packet{Some(Ok(v))=>v,Some(Err(e))=>break e,None=>break Fault::Exited};
                     if !packet.as_object().is_some_and(|m|m.keys().all(|k|matches!(k.as_str(),"kind"|"nonce"|"id"|"ok"|"value"|"error")))||packet["kind"]!="result"||packet["nonce"]!=nonce{break Fault::Protocol;}
                     let Some(id)=packet["id"].as_u64()else{break Fault::Protocol;};let Some(call)=pending.remove(&id)else{break Fault::Protocol;};
                     if *revoked.borrow()||*stop.borrow(){let _=call.reply.send(Err(Fault::Revoked));break Fault::Revoked;}
                     let exact=packet.exact.take();let result=match packet["ok"].as_bool(){Some(true)if packet.get("error").is_none()&&packet.get("value").is_some()=>Ok(OperationResult::Value(packet["value"].clone())),Some(false)if packet.get("value").is_none()&&packet["error"].is_object()=>Ok(OperationResult::NativeError(packet["error"].clone())),_=>{let _=call.reply.send(Err(Fault::Protocol));break Fault::Protocol;}};let result=result.map(|value|exact.unwrap_or(value));let _=call.reply.send(result);
                    }
                   }
        }
    };
    #[cfg(test)]
    if cause == Fault::Deadline {
        if let Some(pid) = child.id() {
            let root = PathBuf::from(format!("/proc/{pid}"));
            let state = std::fs::read_to_string(root.join("stat"))
                .ok()
                .and_then(|s| {
                    s.split(')')
                        .nth(1)
                        .and_then(|s| s.split_whitespace().next())
                        .map(str::to_owned)
                });
            let wait = std::fs::read_to_string(root.join("wchan")).ok();
            let threads = std::fs::read_dir(root.join("task")).ok().map(|v| v.count());
            let argv = std::fs::read(root.join("cmdline")).unwrap_or_default();
            let matched = argv
                .split(|b| *b == 0)
                .any(|v| v == spec.entry.as_os_str().as_encoded_bytes());
            eprintln!(
                "PRIVATE_WORKER_DEADLINE {}",
                json!({"fixture":spec.configuration.get("fixture"),"state":state,"wait":wait,"threads":threads,"entryArgumentMatched":matched})
            );
        }
    }
    owner.revoke();
    for (_, call) in pending {
        let _ = call.reply.send(Err(cause));
    }
    calls.close();
    while let Ok(call) = calls.try_recv() {
        let _ = call.reply.send(Err(cause));
    }
    let _ = writes.try_send(json!({"kind":"close","nonce":nonce}));
    let acknowledgment = tokio::time::timeout(spec.close_deadline, async {
        while let Some(packet) = inbound.recv().await {
            match packet {
                Ok(v)
                    if v["kind"] == "closed"
                        && v["nonce"] == nonce
                        && v["artifactHealthy"] == true
                        && v["executedRecursiveRemovals"] == 0
                        && v.get("guardRefusals").is_none_or(|n| n == 0) =>
                {
                    return Some(v.envelope)
                }
                Ok(v) if v["kind"] == "result" && v["nonce"] == nonce => {}
                _ => return None,
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let clean_exit = matches!(tokio::time::timeout(spec.close_deadline,child.wait()).await,Ok(Ok(status))if status.success());
    let graceful = clean_exit && acknowledgment.is_some();
    let reaped = if clean_exit {
        true
    } else {
        let _ = child.start_kill();
        matches!(
            tokio::time::timeout(spec.close_deadline, child.wait()).await,
            Ok(Ok(_))
        )
    };
    reader.abort();
    writer.abort();
    stderr_task.abort();
    let owner_closed = matches!(
        tokio::time::timeout(spec.close_deadline, owner.close()).await,
        Ok(Ok(()))
    );
    let _ = closed.send(Some(CloseReceipt {
        cause,
        worker_reaped: reaped,
        owner_grant_closed: owner_closed,
        graceful_worker: graceful,
        native_cleanup_requests: acknowledgment
            .as_ref()
            .and_then(|v| v["nativeRecursiveRequests"].as_u64()),
        executed_recursive_removals: acknowledgment
            .as_ref()
            .and_then(|v| v["executedRecursiveRemovals"].as_u64()),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Owner {
        revoked: watch::Sender<bool>,
        closes: AtomicUsize,
        unknown: bool,
    }
    impl Owner {
        fn new(unknown: bool) -> Arc<Self> {
            let (revoked, _) = watch::channel(false);
            Arc::new(Self {
                revoked,
                closes: AtomicUsize::new(0),
                unknown,
            })
        }
    }
    impl GrantOwner for Owner {
        fn target_id(&self) -> &str {
            "synthetic-owned-target"
        }
        fn endpoint(&self) -> &str {
            "ws://127.0.0.1:1/abcdefghijklmnopqrstuvwxyz0123456789"
        }
        fn revocation(&self) -> watch::Receiver<bool> {
            self.revoked.subscribe()
        }
        fn revoke(&self) {
            let _ = self.revoked.send(true);
        }
        fn close(&self) -> Pin<Box<dyn Future<Output = Result<(), Fault>> + Send + '_>> {
            Box::pin(async move {
                self.closes.fetch_add(1, Ordering::SeqCst);
                if self.unknown {
                    Err(Fault::CleanupUnknown)
                } else {
                    Ok(())
                }
            })
        }
    }
    fn spec(mode: &str) -> WorkerSpec {
        crate::test_sandbox::assert_isolated();
        let home = std::env::temp_dir().join(format!(
            "o3-native-page-worker-{}",
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir(&home).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        WorkerSpec {
            node: std::env::var_os("O3_ADMITTED_NODE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/usr/bin/node")),
            entry: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/preview/supervisor-peer.mjs"),
            private_home: home,
            configuration: json!({"fixture":mode}),
            startup_deadline: Duration::from_millis(if mode == "startup-hang" {
                100
            } else {
                2000
            }),
            close_deadline: Duration::from_millis(100),
        }
    }
    #[tokio::test]
    async fn closed_control_before_binding_reaps_the_late_worker() {
        let worker = Worker::spawn(spec("echo"), Owner::new(false))
            .await
            .unwrap();
        let mut closed = worker.handle.closed.clone();
        let control = crate::preview_control::runtime::Control::new(Some("agent".into()));
        control.close().wait().await.unwrap();
        let bound = control::ControlledPage::new(worker, control.clone());
        let receipt = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(receipt) = closed.borrow().clone() {
                    break receipt;
                }
                closed.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(receipt.worker_reaped && receipt.owner_grant_closed);
        assert!(matches!(
            bound
                .agent("agent", "press", json!({}), Duration::from_secs(1), true)
                .await,
            Err(control::Error::Control(_))
        ));
        control.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn actual_control_handoffs_retain_process_and_fence_foreign_agents() {
        let worker = Worker::spawn(spec("echo"), Owner::new(false))
            .await
            .unwrap();
        let control = crate::preview_control::runtime::Control::new(Some("agent".into()));
        let bound = control::ControlledPage::new(worker.clone(), control.clone());
        let OperationResult::Value(first) = bound
            .agent("agent", "press", json!({}), Duration::from_secs(2), true)
            .await
            .unwrap()
        else {
            panic!("Expected first result")
        };
        control.take("viewer").wait().await.unwrap();
        assert!(matches!(
            bound
                .agent("agent", "press", json!({}), Duration::from_secs(2), true)
                .await,
            Err(control::Error::Control(_))
        ));
        control
            .release("viewer", || async { Ok(Value::Null) })
            .wait()
            .await
            .unwrap();
        let OperationResult::Value(second) = bound
            .agent("agent", "press", json!({}), Duration::from_secs(2), true)
            .await
            .unwrap()
        else {
            panic!("Expected second result")
        };
        assert_eq!(first["pid"], second["pid"]);
        assert!(
            second["invalidations"].as_u64().unwrap() > first["invalidations"].as_u64().unwrap()
        );
        assert!(matches!(
            bound
                .agent("foreign", "press", json!({}), Duration::from_secs(2), true)
                .await,
            Err(control::Error::Control(_))
        ));
        assert!(bound.close().await.owner_grant_closed);
        control.close().wait().await.unwrap();
        control.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn javascript_payload_retains_lone_units_without_relaxing_envelope() {
        crate::test_sandbox::assert_isolated();
        let bytes = br#"{"kind":"result","nonce":"valid","id":1,"ok":true,"value":{"\ud800":"\udfff","pair":"\ud83d\ude00"}}"#;
        let packet = decode_packet(bytes).unwrap();
        let Some(OperationResult::JavaScriptValue(exact)) = packet.exact else {
            panic!("Expected exact UTF-16 JSON")
        };
        assert_eq!(exact.json(), r#"{"\ud800":"\udfff","pair":"\ud83d\ude00"}"#);
        assert_eq!(serde_json::to_string(&exact).unwrap(), exact.json());
        assert!(serde_json::from_str::<Value>(exact.json()).is_err());
        for bad in [
            br#"{"kind":"result","nonce":"\ud800","id":1,"ok":true,"value":"\udfff"}"#.as_slice(),
            br#"{"kind":"ready","nonce":"valid","value":"\udfff"}"#.as_slice(),
            br#"{"kind":"result","nonce":"valid","id":1,"ok":false,"error":"\udfff"}"#.as_slice(),
            br#"{"kind":"result","nonce":"valid","id":1,"ok":true,"value":"\uQQQQ"}"#.as_slice(),
        ] {
            assert!(matches!(decode_packet(bad), Err(Fault::Protocol)));
        }
        let worker = Worker::spawn(spec("surrogate"), Owner::new(false))
            .await
            .unwrap();
        let OperationResult::JavaScriptValue(result) = worker
            .invoke("evaluate", json!({}), Duration::from_secs(1))
            .await
            .unwrap()
        else {
            panic!("Lost JavaScript units")
        };
        assert!(result.json().contains(r#""\ud800":"\udfff""#));
        assert!(worker.close().await.graceful_worker);
    }
    #[tokio::test]
    async fn native_response_grace_does_not_expand_operation_timeout() {
        let owner = Owner::new(false);
        let worker = Worker::spawn(spec("grace"), owner).await.unwrap();
        assert_eq!(
            worker
                .invoke_native("evaluate", json!({}), Duration::from_millis(20), true)
                .await
                .unwrap(),
            OperationResult::Value(json!({"nativeTimeoutMs":20}))
        );
        assert!(worker.close().await.owner_grant_closed);
        let worker = Worker::spawn(spec("grace"), Owner::new(false))
            .await
            .unwrap();
        assert_eq!(
            worker
                .invoke_native("evaluate", json!({}), Duration::from_millis(20), false)
                .await
                .unwrap_err(),
            Fault::Deadline
        );
        assert!(worker.close().await.worker_reaped);
    }
    #[tokio::test]
    async fn real_owned_node_protocol_and_native_errors() {
        let owner = Owner::new(false);
        let worker = Worker::spawn(spec("echo"), owner.clone()).await.unwrap();
        let OperationResult::Value(value) = worker
            .invoke("press", json!({"key":"A"}), Duration::from_secs(1))
            .await
            .unwrap()
        else {
            panic!("Expected value")
        };
        assert_eq!(value["operation"], "press");
        assert_eq!(value["ambient"], Value::Null);
        let receipt = worker.close().await;
        assert!(receipt.worker_reaped && receipt.owner_grant_closed && receipt.graceful_worker);
        assert_eq!(owner.closes.load(Ordering::SeqCst), 1);
        assert_eq!(
            worker
                .invoke("press", json!({}), Duration::from_secs(1))
                .await
                .unwrap_err(),
            Fault::Closed
        );
        let worker = Worker::spawn(spec("native-error"), Owner::new(false))
            .await
            .unwrap();
        assert!(matches!(
            worker
                .invoke("click", json!({}), Duration::from_secs(1))
                .await
                .unwrap(),
            OperationResult::NativeError(_)
        ));
        assert!(worker.close().await.owner_grant_closed);
    }
    #[tokio::test]
    async fn malformed_mixed_oversized_and_crashed_children_are_reaped() {
        for mode in ["mixed", "oversize", "wrong-nonce", "startup-hang"] {
            let owner = Owner::new(false);
            let error = match Worker::spawn(spec(mode), owner.clone()).await {
                Err(error) => error,
                Ok(_) => panic!("Invalid startup accepted"),
            };
            assert!(error.worker_reaped && error.owner_grant_closed, "{mode}");
            assert_eq!(owner.closes.load(Ordering::SeqCst), 1);
        }
        for mode in ["malformed", "crash"] {
            let worker = Worker::spawn(spec(mode), Owner::new(false)).await.unwrap();
            assert!(worker
                .invoke("click", json!({}), Duration::from_secs(1))
                .await
                .is_err());
            assert!(worker.close().await.worker_reaped);
        }
    }
    #[tokio::test]
    async fn late_timeout_revocation_and_unknown_owner_are_not_success() {
        for mode in ["late", "hang"] {
            let setup = spec(mode);
            let marker = setup.private_home.join("peer-started.json");
            let late_marker = setup.private_home.join("late-sent.json");
            let worker = Worker::spawn(setup, Owner::new(false))
                .await
                .unwrap_or_else(|failure| {
                    panic!(
                        "{mode}: {failure:?}; peer={:?}",
                        std::fs::read_to_string(marker)
                    )
                });
            assert_eq!(
                worker
                    .invoke("type", json!({}), Duration::from_millis(20))
                    .await
                    .unwrap_err(),
                Fault::Deadline
            );
            let receipt = worker.close().await;
            assert!(receipt.worker_reaped && receipt.owner_grant_closed);
            if mode == "late" {
                assert!(late_marker.is_file());
                assert_eq!(
                    worker
                        .invoke("press", json!({}), Duration::from_secs(1))
                        .await
                        .unwrap_err(),
                    Fault::Closed
                );
            }
        }
        let owner = Owner::new(false);
        let worker = Worker::spawn(spec("hang"), owner.clone()).await.unwrap();
        let call = worker.clone();
        let waiting = tokio::spawn(async move {
            call.invoke("waitFor", json!({}), Duration::from_secs(2))
                .await
        });
        tokio::task::yield_now().await;
        owner.revoked.send(true).unwrap();
        assert!(waiting.await.unwrap().is_err());
        let receipt = worker.close().await;
        assert!(receipt.worker_reaped && receipt.owner_grant_closed);
        let worker = Worker::spawn(spec("echo"), Owner::new(true)).await.unwrap();
        let receipt = worker.close().await;
        assert!(receipt.worker_reaped);
        assert!(!receipt.owner_grant_closed);
    }
    #[tokio::test]
    async fn oversized_requests_and_queue_pressure_are_bounded() {
        let worker = Worker::spawn(spec("echo"), Owner::new(false))
            .await
            .unwrap();
        assert_eq!(
            worker
                .invoke(
                    "type",
                    json!({"text":"x".repeat(MAX_FRAME_BYTES)}),
                    Duration::from_secs(2)
                )
                .await
                .unwrap_err(),
            Fault::TooLarge
        );
        assert!(matches!(
            worker
                .invoke("press", json!({"key":"a"}), Duration::from_secs(1))
                .await
                .unwrap(),
            OperationResult::Value(_)
        ));
        assert!(worker.close().await.worker_reaped);
        let worker = Worker::spawn(spec("hang"), Owner::new(false))
            .await
            .unwrap();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..24 {
            let call = worker.clone();
            tasks.spawn(async move {
                call.invoke("waitFor", json!({}), Duration::from_secs(3))
                    .await
            });
        }
        let busy = tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(result) = tasks.join_next().await {
                if result.unwrap() == Err(Fault::Busy) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap();
        assert!(busy);
        let receipt = worker.close().await;
        assert!(receipt.worker_reaped && receipt.owner_grant_closed);
        while tasks.join_next().await.is_some() {}
    }
}
