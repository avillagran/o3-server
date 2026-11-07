//! Owned native speech transport. This module does not enable a speech service.
mod frame;
mod process;
mod seal;
pub use process::Cleanup;
pub use seal::{EnginePackage, SealedFile};

use frame::Reader;
use process::OwnedProcess;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::{mpsc, oneshot, watch},
};

const BRIDGE: &str = include_str!("../runtime/speech/worker.mjs");
const NATIVE: &[u8] = include_bytes!("../runtime/speech/native-worker.mjs");
const MANIFEST: &[u8] = include_bytes!("../runtime/speech/native-manifest.json");

/// A private immutable executable snapshot, shared across workers without
/// repeatedly copying a150MB runtime. The acquisition owner retains this object.
pub struct NodeRuntime {
    file: SealedFile,
}
impl NodeRuntime {
    pub fn capture(file: SealedFile, destination: &Path) -> io::Result<Arc<Self>> {
        if !file.is_executable()? {
            return Err(io::Error::other("Invalid Node executable"));
        }
        Ok(Arc::new(Self {
            file: file.snapshot(destination, true)?,
        }))
    }
}

/// Only trusted application owners may construct this. There is no wire decoder.
pub struct WorkerSpec {
    pub node: Arc<NodeRuntime>,
    pub engine: EnginePackage,
    pub model: Option<SealedFile>,
    pub vad: Option<SealedFile>,
    /// A unique, nonexistent private runtime directory, retained after shutdown.
    pub directory: PathBuf,
    pub acceleration: String,
    pub startup_deadline: Duration,
    pub operation_deadline: Duration,
    pub close_deadline: Duration,
}

/// A failed runtime startup still carries its explicit terminal receipt. Callers
/// must not infer successful cleanup merely from receiving an I/O error.
#[derive(Debug)]
pub struct StartupFailure {
    pub cleanup: Cleanup,
}
impl std::fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Owned speech startup failed; cleanup: {:?}",
            self.cleanup
        )
    }
}
impl std::error::Error for StartupFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Busy,
    Closed,
    Stale,
    Protocol,
    InvalidInput,
    CleanupUnknown,
    Native(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    generation: u64,
    worker: [u8; 16],
}

struct State {
    busy: bool,
    next: u64,
    stream: Option<u64>,
    closed: bool,
}
struct Shared {
    identity: [u8; 16],
    state: Mutex<State>,
    commands: mpsc::UnboundedSender<Request>,
    cancel: mpsc::UnboundedSender<u64>,
    stop: watch::Sender<bool>,
    cleanup: watch::Receiver<Option<Cleanup>>,
}
impl Drop for Shared {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}
#[derive(Clone)]
pub struct Worker {
    shared: Arc<Shared>,
}
struct Request {
    id: u64,
    frame: Value,
    reply: oneshot::Sender<Result<Value, Error>>,
}
struct Reservation {
    shared: Arc<Shared>,
    id: u64,
    armed: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.shared.cancel.send(self.id);
        }
    }
}

fn random_identity() -> io::Result<[u8; 16]> {
    let mut identity = [0; 16];
    getrandom::fill(&mut identity).map_err(io::Error::other)?;
    Ok(identity)
}

fn create(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

struct Prepared {
    spec: WorkerSpec,
    owner: String,
    command: Command,
}
fn prepare(spec: WorkerSpec) -> io::Result<Prepared> {
    if !spec.node.file.is_executable()?
        || crate::managed_settings_codec::decode_patch(
            json!({"speechAcceleration":&spec.acceleration}),
        )
        .is_err()
    {
        return Err(io::Error::other("Invalid owned speech runtime"));
    }
    // A fresh child of an admitted private directory; no existing tree is altered.
    let parent = spec
        .directory
        .parent()
        .ok_or_else(|| io::Error::other("Missing speech parent"))?;
    let _parent_lease = seal::directory(parent)?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&spec.directory)?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(spec.directory.join("home"))?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(spec.directory.join("tmp"))?;

    let engine = spec.engine.snapshot(&spec.directory.join("engine"))?;
    let model = spec
        .model
        .as_ref()
        .map(|file| file.snapshot(&spec.directory.join("model"), false))
        .transpose()?;
    let vad = spec
        .vad
        .as_ref()
        .map(|file| file.snapshot(&spec.directory.join("vad"), false))
        .transpose()?;
    let owner = crate::store::random_id().map_err(io::Error::other)?;
    let document = json!({"version":1,"owner":owner,"nativeSha256":format!("{:x}",Sha256::digest(NATIVE)),
            "engine":engine.receipt(),"model":model.as_ref().map(SealedFile::receipt),"vad":vad.as_ref().map(SealedFile::receipt),"acceleration":spec.acceleration});
    let bytes = serde_json::to_vec(&document)?;
    create(&spec.directory.join("spec.json"), &bytes)?;
    create(&spec.directory.join("native-worker.mjs"), NATIVE)?;
    create(&spec.directory.join("native-manifest.json"), MANIFEST)?;
    use base64::Engine;
    let native_url = format!(
        "data:text/javascript;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(NATIVE)
    );
    let admitted = serde_json::to_string(&spec.engine.paths(&spec.directory.join("engine")))?;
    let guard=format!("import {{registerHooks,isBuiltin}} from 'node:module';\nimport {{fileURLToPath}} from 'node:url';\nconst files=new Set({admitted});\nconst native={};\nregisterHooks({{resolve(specifier,context,next){{const result=next(specifier,context);if(isBuiltin(result.url)||result.url===native)return result;if(result.url.startsWith('file:')&&files.has(fileURLToPath(result.url)))return result;throw Error('Unadmitted speech package import');}}}});\n",serde_json::to_string(&native_url)?);
    let guard_path = spec.directory.join("import-guard.mjs");
    create(&guard_path, guard.as_bytes())?;
    let guard_url = url::Url::from_file_path(&guard_path)
        .map_err(|_| io::Error::other("Invalid speech guard path"))?;
    let mut command = Command::new(spec.node.file.path());
    command
        .current_dir(&spec.directory)
        .args(["--input-type=module", "-e", BRIDGE, "--", "owned-speech"])
        .arg(&spec.directory)
        .arg(format!("{:x}", Sha256::digest(&bytes)))
        .env_clear()
        .env("HOME", spec.directory.join("home"))
        .env("TMPDIR", spec.directory.join("tmp"))
        .env("PATH", "")
        .env("NODE_OPTIONS", format!("--import={guard_url}"))
        .env("ELECTRON_RUN_AS_NODE", "1");
    Ok(Prepared {
        spec,
        owner,
        command,
    })
}
impl Worker {
    pub async fn start(spec: WorkerSpec) -> io::Result<Self> {
        // Reserve fallible local state before any process exists, so an entropy
        // error cannot bypass the post-spawn terminal cleanup receipt.
        let identity = random_identity()?;
        let Prepared {
            spec,
            owner,
            mut command,
        } = tokio::task::spawn_blocking(move || prepare(spec))
            .await
            .map_err(io::Error::other)??;
        let mut process = OwnedProcess::spawn(&mut command)?;
        let (input, output) = match process.take_pipes() {
            Ok(pipes) => pipes,
            Err(_) => {
                let cleanup = process.close(async { false }, spec.close_deadline).await;
                return Err(io::Error::other(StartupFailure { cleanup }));
            }
        };
        let mut reader = Reader::new(output);
        let hello = tokio::time::timeout(spec.startup_deadline, reader.next()).await;
        if !matches!(&hello, Ok(Ok(Some(value))) if value.get("type")==Some(&json!("hello")) && value.get("owner")==Some(&json!(owner)) && value.get("node")==Some(&json!("26.13.0")) && value.get("pid").and_then(Value::as_u64)==Some(u64::from(process.id())))
        {
            let cleanup = process.close(async { false }, spec.close_deadline).await;
            return Err(io::Error::other(StartupFailure { cleanup }));
        }
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (cancel, cancel_rx) = mpsc::unbounded_channel();
        let (stop, stop_rx) = watch::channel(false);
        let (cleanup_tx, cleanup) = watch::channel(None);
        let shared = Arc::new(Shared {
            identity,
            state: Mutex::new(State {
                busy: false,
                next: 1,
                stream: None,
                closed: false,
            }),
            commands,
            cancel,
            stop,
            cleanup,
        });
        let weak = Arc::downgrade(&shared);
        tokio::spawn(async move {
            Actor {
                process,
                input,
                reader,
                owner,
                commands: command_rx,
                cancel: cancel_rx,
                stop: stop_rx,
                deadline: spec.operation_deadline,
                close: spec.close_deadline,
            }
            .run()
            .await
            .publish(weak, cleanup_tx);
            // Keep all admitted leases alive through the terminal group fence.
            drop(spec);
        });
        Ok(Self { shared })
    }

    async fn request(
        &self,
        build: impl FnOnce() -> Value,
        stream: Option<Stream>,
        begin: bool,
        finish: bool,
        terminal: bool,
    ) -> Result<(Value, u64), Error> {
        let (reply, receiver) = oneshot::channel();
        let id = {
            let mut state = self.shared.state.lock().map_err(|_| Error::Closed)?;
            if state.closed {
                return Err(Error::Closed);
            }
            if state.busy {
                return Err(Error::Busy);
            }
            if let Some(stream) = stream {
                if stream.worker != self.shared.identity || state.stream != Some(stream.generation)
                {
                    return Err(Error::Stale);
                }
            } else if state.stream.is_some() {
                return Err(Error::Busy);
            }
            let id = state.next;
            state.next = id
                .checked_add(1)
                .filter(|value| *value <= 9_007_199_254_740_991)
                .ok_or(Error::Closed)?;
            state.busy = true;
            if terminal {
                state.closed = true;
            }
            if begin {
                state.stream = Some(id);
            }
            id
        };
        let mut reservation = Reservation {
            shared: self.shared.clone(),
            id,
            armed: true,
        };
        let mut frame = build();
        frame["id"] = json!(id);
        self.shared
            .commands
            .send(Request { id, frame, reply })
            .map_err(|_| Error::Closed)?;
        let result = receiver.await.map_err(|_| Error::Closed)?;
        let mut state = self.shared.state.lock().map_err(|_| Error::Closed)?;
        state.busy = false;
        if finish || (begin && result.is_err()) {
            state.stream = None;
        }
        reservation.armed = false;
        result.map(|value| (value, id))
    }

    /// Discovery owns a one-shot worker: return only after its complete process
    /// group is fenced and its leader reaped. A Busy peer is never cancelled.
    pub async fn discover(&self) -> Result<Value, Error> {
        let result = self
            .request(|| json!({"kind":"discover"}), None, false, false, true)
            .await;
        if matches!(result, Err(Error::Busy | Error::Closed | Error::Stale)) {
            return result.map(|x| x.0);
        }
        match self.close().await {
            Cleanup::Confirmed { .. } => result.map(|x| x.0),
            Cleanup::Unknown => Err(Error::CleanupUnknown),
        }
    }

    pub async fn load(&self) -> Result<Value, Error> {
        self.request(|| json!({"kind":"load"}), None, false, false, false)
            .await
            .map(|x| x.0)
    }
    pub async fn transcribe(
        &self,
        pcm: &crate::speech_kernel::Pcm,
        options: Value,
        filter_silence: bool,
    ) -> Result<Value, Error> {
        use base64::Engine;
        self.request(||json!({"kind":"transcribe","pcm":base64::engine::general_purpose::STANDARD.encode(pcm.samples().iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>()),"options":options,"filterSilence":filter_silence}),None,false,false,false).await.map(|x|x.0)
    }
    pub async fn begin(&self, options: Value, filter_silence: bool) -> Result<Stream, Error> {
        self.request(
            || json!({"kind":"begin","options":options,"filterSilence":filter_silence}),
            None,
            true,
            false,
            false,
        )
        .await
        .map(|x| Stream {
            generation: x.1,
            worker: self.shared.identity,
        })
    }
    pub async fn feed(
        &self,
        stream: Stream,
        pcm: &crate::speech_kernel::Pcm,
    ) -> Result<Value, Error> {
        use base64::Engine;
        if pcm.samples().len() > 16_000 {
            return Err(Error::InvalidInput);
        }
        self.request(||json!({"kind":"feed","pcm":base64::engine::general_purpose::STANDARD.encode(pcm.samples().iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>())}),Some(stream),false,false,false).await.map(|x|x.0)
    }
    pub async fn reset(&self, stream: Stream) -> Result<Value, Error> {
        self.request(|| json!({"kind":"reset"}), Some(stream), false, true, false)
            .await
            .map(|x| x.0)
    }
    pub async fn finish(&self, stream: Stream) -> Result<Value, Error> {
        self.request(
            || json!({"kind":"finish"}),
            Some(stream),
            false,
            true,
            false,
        )
        .await
        .map(|x| x.0)
    }
    pub async fn close(&self) -> Cleanup {
        if let Ok(mut state) = self.shared.state.lock() {
            state.closed = true;
        }
        let _ = self.shared.stop.send(true);
        let mut receipt = self.shared.cleanup.clone();
        loop {
            if let Some(value) = *receipt.borrow_and_update() {
                return value;
            }
            if receipt.changed().await.is_err() {
                return Cleanup::Unknown;
            }
        }
    }
}

struct Outcome(Cleanup);
impl Outcome {
    fn publish(self, shared: std::sync::Weak<Shared>, sender: watch::Sender<Option<Cleanup>>) {
        if let Some(shared) = shared.upgrade() {
            if let Ok(mut state) = shared.state.lock() {
                state.closed = true;
                state.busy = false;
                state.stream = None;
            }
        }
        let _ = sender.send(Some(self.0));
    }
}

struct Actor {
    process: OwnedProcess,
    input: tokio::process::ChildStdin,
    reader: Reader<tokio::process::ChildStdout>,
    owner: String,
    commands: mpsc::UnboundedReceiver<Request>,
    cancel: mpsc::UnboundedReceiver<u64>,
    stop: watch::Receiver<bool>,
    deadline: Duration,
    close: Duration,
}
impl Actor {
    async fn run(self) -> Outcome {
        let Self {
            process,
            mut input,
            mut reader,
            owner,
            mut commands,
            mut cancel,
            mut stop,
            deadline,
            close,
        } = self;
        let mut generation = None;
        let mut completed = None;
        loop {
            let request = tokio::select! {
                biased;
                _=stop.changed()=>break,
                value=cancel.recv()=>{ match value {Some(id) if generation == Some(id) || completed == Some(id)=>break,None=>break,_=>continue} },
                value=commands.recv()=>match value {Some(value)=>value,None=>break},
                _=reader.next()=>break,
            };
            let kind = request
                .frame
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let mut frame = request.frame;
            frame["owner"] = json!(owner);
            let mut bytes = match serde_json::to_vec(&frame) {
                Ok(bytes) => bytes,
                Err(_) => break,
            };
            bytes.push(b'\n');
            if bytes.len() > frame::MAX_RECORD {
                let _ = request.reply.send(Err(Error::Protocol));
                break;
            }
            let operation = async {
                input.write_all(&bytes).await.map_err(|_| Error::Closed)?;
                let value = reader
                    .next()
                    .await
                    .map_err(|_| Error::Protocol)?
                    .ok_or(Error::Closed)?;
                if value.get("type") != Some(&json!("reply"))
                    || value.get("owner") != Some(&json!(owner))
                    || value.get("id").and_then(Value::as_u64) != Some(request.id)
                {
                    return Err(Error::Protocol);
                }
                match value.get("ok").and_then(Value::as_bool) {
                    Some(true) => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                    Some(false) => Err(Error::Native(
                        value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("Native speech failed.")
                            .to_owned(),
                    )),
                    None => Err(Error::Protocol),
                }
            };
            let cancelled = async {
                loop {
                    match cancel.recv().await {
                        Some(id) if id == request.id || generation == Some(id) => break,
                        None => break,
                        _ => {}
                    }
                }
            };
            let result = tokio::select! {
                biased;
                _=stop.changed()=>Err(Error::Closed),
                _=cancelled=>Err(Error::Closed),
                value=tokio::time::timeout(deadline,operation)=>value.unwrap_or(Err(Error::Closed)),
            };
            let fatal = matches!(result, Err(Error::Closed | Error::Protocol));
            if result.is_ok() {
                if kind == "begin" {
                    generation = Some(request.id);
                }
                if kind == "finish" {
                    generation = None;
                }
            }
            completed = Some(request.id);
            let _ = request.reply.send(result);
            if fatal {
                break;
            }
        }
        drop(input);
        let grace = async {
            loop {
                match reader.next().await {
                    Ok(None) => return true,
                    Ok(Some(_)) => {}
                    Err(_) => return false,
                }
            }
        };
        Outcome(process.close(grace, close).await)
    }
}
