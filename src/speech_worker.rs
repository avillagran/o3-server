//! Owned native speech transport. This module does not enable a speech service.
mod frame;
mod process;
mod seal;
mod thread_open;
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
const TEXT_MANIFEST: &[u8] = include_bytes!("../runtime/speech/text/manifest.json");
const TEXT_MODULES: &[(&str, &[u8])] = &[
    (
        "transcription-post-processing.mjs",
        include_bytes!("../runtime/speech/text/transcription-post-processing.mjs"),
    ),
    (
        "custom-words.mjs",
        include_bytes!("../runtime/speech/text/custom-words.mjs"),
    ),
    (
        "filler-words.mjs",
        include_bytes!("../runtime/speech/text/filler-words.mjs"),
    ),
    (
        "native-language.mjs",
        include_bytes!("../runtime/speech/text/native-language.mjs"),
    ),
    (
        "model.mjs",
        include_bytes!("../runtime/speech/text/model.mjs"),
    ),
];

/// A private immutable executable snapshot, shared across workers without
/// repeatedly copying a150MB runtime. The acquisition owner retains this object.
pub struct NodeRuntime {
    file: SealedFile,
}
impl NodeRuntime {
    /// Reuse only an exclusive, private, read-only snapshot with the admitted
    /// digest. An unknown/partial destination is retained and never overwritten.
    pub fn capture_or_reuse(file: SealedFile, destination: &Path) -> io::Result<Arc<Self>> {
        let receipt = file.receipt();
        match std::fs::symlink_metadata(destination) {
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => {
                Self::capture(file, destination)
            }
            Err(cause) => Err(cause),
            Ok(_) => {
                let _parent = seal::directory(
                    destination
                        .parent()
                        .ok_or_else(|| io::Error::other("Missing private Node parent"))?,
                )?;
                let snapshot = SealedFile::admit(
                    destination,
                    receipt["sha256"].as_str().unwrap(),
                    receipt["bytes"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .map_err(io::Error::other)?,
                )?;
                if !snapshot.is_private_immutable_executable()? {
                    return Err(io::Error::other(
                        "Node snapshot is not private and immutable",
                    ));
                }
                Ok(Arc::new(Self { file: snapshot }))
            }
        }
    }
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

/// The text-only resource has no engine, model, VAD, package URL, or import authority.
pub struct TextWorkerSpec {
    pub node: Arc<NodeRuntime>,
    pub directory: PathBuf,
    pub startup_deadline: Duration,
    pub operation_deadline: Duration,
    pub close_deadline: Duration,
}
struct RuntimeSpec {
    node: Arc<NodeRuntime>,
    engine: Option<EnginePackage>,
    model: Option<SealedFile>,
    vad: Option<SealedFile>,
    directory: PathBuf,
    acceleration: String,
    startup_deadline: Duration,
    operation_deadline: Duration,
    close_deadline: Duration,
}
impl From<WorkerSpec> for RuntimeSpec {
    fn from(spec: WorkerSpec) -> Self {
        Self {
            node: spec.node,
            engine: Some(spec.engine),
            model: spec.model,
            vad: spec.vad,
            directory: spec.directory,
            acceleration: spec.acceleration,
            startup_deadline: spec.startup_deadline,
            operation_deadline: spec.operation_deadline,
            close_deadline: spec.close_deadline,
        }
    }
}
impl From<TextWorkerSpec> for RuntimeSpec {
    fn from(spec: TextWorkerSpec) -> Self {
        Self {
            node: spec.node,
            engine: None,
            model: None,
            vad: None,
            directory: spec.directory,
            acceleration: "cpu".into(),
            startup_deadline: spec.startup_deadline,
            operation_deadline: spec.operation_deadline,
            close_deadline: spec.close_deadline,
        }
    }
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
    attachment: Option<SealedFile>,
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

fn cancelled_now(cancel: &Option<watch::Receiver<bool>>) -> bool {
    cancel
        .as_ref()
        .is_some_and(|cancel| *cancel.borrow() || cancel.has_changed().is_err())
}
async fn wait_cancel(cancel: &mut Option<watch::Receiver<bool>>) {
    if let Some(cancel) = cancel {
        loop {
            if *cancel.borrow() || cancel.changed().await.is_err() {
                return;
            }
        }
    } else {
        std::future::pending::<()>().await;
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
    spec: RuntimeSpec,
    owner: String,
    command: Command,
}
fn prepare(spec: RuntimeSpec) -> io::Result<Prepared> {
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

    let engine = spec
        .engine
        .as_ref()
        .map(|engine| engine.snapshot(&spec.directory.join("engine")))
        .transpose()?;
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
            "mode":if spec.engine.is_some(){"speech"}else{"text"},"textManifestSha256":format!("{:x}",Sha256::digest(TEXT_MANIFEST)),"engine":engine.as_ref().map(SealedFile::receipt),"model":model.as_ref().map(SealedFile::receipt),"vad":vad.as_ref().map(SealedFile::receipt),"acceleration":spec.acceleration});
    let bytes = serde_json::to_vec(&document)?;
    create(&spec.directory.join("spec.json"), &bytes)?;
    create(&spec.directory.join("native-worker.mjs"), NATIVE)?;
    create(&spec.directory.join("native-manifest.json"), MANIFEST)?;
    create(&spec.directory.join("text-manifest.json"), TEXT_MANIFEST)?;
    for (name, bytes) in TEXT_MODULES {
        create(&spec.directory.join(format!("text-{name}")), bytes)?;
    }
    use base64::Engine;
    let native_url = format!(
        "data:text/javascript;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(NATIVE)
    );
    let mut trusted = vec![native_url];
    for (_, bytes) in TEXT_MODULES {
        trusted.push(format!(
            "data:text/javascript;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
    }
    let trusted = serde_json::to_string(&trusted)?;
    let admitted = serde_json::to_string(
        &spec
            .engine
            .as_ref()
            .map(|engine| engine.paths(&spec.directory.join("engine")))
            .unwrap_or_default(),
    )?;
    let guard=format!("import {{registerHooks,isBuiltin}} from 'node:module';\nimport {{fileURLToPath}} from 'node:url';\nconst files=new Set({admitted});\nconst trusted=new Set({trusted});\nregisterHooks({{resolve(specifier,context,next){{const result=next(specifier,context);if(isBuiltin(result.url)||trusted.has(result.url))return result;if(result.url.startsWith('file:')&&files.has(fileURLToPath(result.url)))return result;throw Error('Unadmitted speech package import');}}}});\n");
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
        Self::start_owned(spec.into(), None).await
    }
    /// A trusted owner may cancel admission/startup without abandoning the
    /// post-spawn process receipt. Pre-spawn private preparation is retained.
    pub async fn start_cancellable(
        spec: WorkerSpec,
        cancel: watch::Receiver<bool>,
    ) -> io::Result<Self> {
        Self::start_owned(spec.into(), Some(cancel)).await
    }
    pub async fn start_text(
        spec: TextWorkerSpec,
        cancel: watch::Receiver<bool>,
    ) -> io::Result<Self> {
        Self::start_owned(spec.into(), Some(cancel)).await
    }
    async fn start_owned(
        spec: RuntimeSpec,
        mut cancel: Option<watch::Receiver<bool>>,
    ) -> io::Result<Self> {
        if cancelled_now(&cancel) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Owned speech startup cancelled",
            ));
        }
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
        if cancelled_now(&cancel) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Owned speech startup cancelled",
            ));
        }
        let mut process = OwnedProcess::spawn(&mut command)?;
        let (input, output) = match process.take_pipes() {
            Ok(pipes) => pipes,
            Err(_) => {
                let cleanup = process.close(async { false }, spec.close_deadline).await;
                return Err(io::Error::other(StartupFailure { cleanup }));
            }
        };
        let mut reader = Reader::new(output);
        let hello = tokio::select! {biased;
            _=wait_cancel(&mut cancel)=>None,
            value=tokio::time::timeout(spec.startup_deadline,reader.next())=>Some(value),
        };
        if !matches!(&hello, Some(Ok(Ok(Some(value)))) if value.get("type")==Some(&json!("hello")) && value.get("owner")==Some(&json!(owner)) && value.get("node")==Some(&json!("26.13.0")) && value.get("pid").and_then(Value::as_u64)==Some(u64::from(process.id())))
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
                directory: spec.directory.clone(),
                vad_signature: spec.vad.as_ref().map(|file| {
                    let value = file.receipt();
                    (
                        value["sha256"].as_str().unwrap().to_owned(),
                        value["bytes"].as_str().unwrap().to_owned(),
                    )
                }),
            }
            .run()
            .await
            .publish(weak, cleanup_tx);
            // Keep all admitted leases alive through the terminal group fence.
            drop(spec);
        });
        Ok(Self { shared })
    }

    pub async fn text_ready(&self) -> Result<(), Error> {
        let value = self
            .request(|| json!({"kind":"text-ready"}), None, false, false, false)
            .await?
            .0;
        if value == true {
            Ok(())
        } else {
            Err(Error::Protocol)
        }
    }
    pub async fn correct_cloud(
        &self,
        text: &str,
        options: &crate::speech_request::TranscriptionOptions,
        language: &str,
    ) -> Result<String, Error> {
        self.request(|| json!({"kind":"cloud-correct","text":text,"options":options.as_value(),"language":language}),None,false,false,false).await?.0.as_str().map(str::to_owned).ok_or(Error::Protocol)
    }
    /// Preserve exact UTF-16 units when native slicing cuts an astral pair.
    pub async fn postprocess_prompt(
        &self,
        instructions: &str,
        transcript: &str,
        draft: &Value,
        correction: &str,
    ) -> Result<Vec<u16>, Error> {
        let result=self.request(||json!({"kind":"postprocess-prompt","instructions":instructions,"transcript":transcript,"draft":draft,"correction":correction}),None,false,false,false).await?.0;
        result
            .as_array()
            .ok_or(Error::Protocol)?
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|unit| u16::try_from(unit).ok())
                    .ok_or(Error::Protocol)
            })
            .collect()
    }

    pub async fn correct_postprocess(
        &self,
        text: &str,
        original: &str,
        words: &Value,
    ) -> Result<String, Error> {
        self.request(
            || json!({"kind":"postprocess-correct","text":text,"original":original,"words":words}),
            None,
            false,
            false,
            false,
        )
        .await?
        .0
        .as_str()
        .map(str::to_owned)
        .ok_or(Error::Protocol)
    }

    async fn request(
        &self,
        build: impl FnOnce() -> Value,
        stream: Option<Stream>,
        begin: bool,
        finish: bool,
        terminal: bool,
    ) -> Result<(Value, u64), Error> {
        self.request_owned(build, stream, begin, finish, terminal, None)
            .await
    }
    async fn request_owned(
        &self,
        build: impl FnOnce() -> Value,
        stream: Option<Stream>,
        begin: bool,
        finish: bool,
        terminal: bool,
        attachment: Option<SealedFile>,
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
            .send(Request {
                id,
                frame,
                reply,
                attachment,
            })
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
    /// Owner-only late sidecar binding. The actor snapshots admitted bytes under
    /// its existing reservation and retains that immutable lease through fencing.
    /// Rebinding the same digest is a no-op; a different sidecar is refused.
    pub async fn bind_vad(&self, file: SealedFile) -> Result<(), Error> {
        self.request_owned(
            || json!({"kind":"vad"}),
            None,
            false,
            false,
            false,
            Some(file),
        )
        .await
        .map(|_| ())
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
    /// Buffered service request with the complete native text/language helpers.
    /// Options have already passed the exact native request codec.
    pub async fn transcribe_options(
        &self,
        pcm: &crate::speech_kernel::Pcm,
        model_id: &str,
        options: &crate::speech_request::TranscriptionOptions,
    ) -> Result<Value, Error> {
        use base64::Engine;
        self.request(||json!({"kind":"service-transcribe","pcm":base64::engine::general_purpose::STANDARD.encode(pcm.samples().iter().flat_map(|value|value.to_le_bytes()).collect::<Vec<_>>()),"modelId":model_id,"options":options.as_value()}),None,false,false,false).await.map(|value|value.0)
    }

    pub async fn begin_options(
        &self,
        model_id: &str,
        options: &crate::speech_request::TranscriptionOptions,
    ) -> Result<Stream, Error> {
        if crate::speech_catalog::get(model_id).is_none() {
            return Err(Error::InvalidInput);
        }
        self.request(
            || json!({"kind":"service-begin","modelId":model_id,"options":options.as_value()}),
            None,
            true,
            false,
            false,
        )
        .await
        .map(|result| Stream {
            generation: result.1,
            worker: self.shared.identity,
        })
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
    directory: PathBuf,
    vad_signature: Option<(String, String)>,
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
            directory,
            mut vad_signature,
        } = self;
        let mut leases = Vec::new();
        let mut preparation: Option<tokio::task::JoinHandle<io::Result<SealedFile>>> = None;
        let mut model_loaded = false;
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
            let mut new_signature = None;
            if let Some(source) = request.attachment {
                if !model_loaded {
                    completed = Some(request.id);
                    let _ = request.reply.send(Err(Error::InvalidInput));
                    continue;
                }
                let receipt = source.receipt();
                let signature = (
                    receipt["sha256"].as_str().unwrap().to_owned(),
                    receipt["bytes"].as_str().unwrap().to_owned(),
                );
                if let Some(existing) = &vad_signature {
                    completed = Some(request.id);
                    let _ = request.reply.send(if existing == &signature {
                        Ok(Value::Null)
                    } else {
                        Err(Error::InvalidInput)
                    });
                    continue;
                }
                let bounded = signature
                    .1
                    .parse::<u64>()
                    .is_ok_and(|size| size > 0 && size <= 2 * 1024 * 1024);
                let id = crate::store::random_id();
                if !bounded || id.is_err() {
                    completed = Some(request.id);
                    let _ = request.reply.send(Err(Error::InvalidInput));
                    continue;
                }
                let destination = directory.join(format!("vad-{}", id.unwrap()));
                preparation = Some(tokio::task::spawn_blocking(move || {
                    source.snapshot(&destination, false)
                }));
                new_signature = Some(signature);
            }
            let operation = async {
                if let Some(job) = preparation.as_mut() {
                    let file = match job.await {
                        Ok(Ok(file)) => file,
                        _ => {
                            preparation = None;
                            return Err(Error::InvalidInput);
                        }
                    };
                    preparation = None;
                    frame["vad"] = file.receipt();
                    leases.push(file);
                }
                let mut bytes = serde_json::to_vec(&frame).map_err(|_| Error::Protocol)?;
                bytes.push(b'\n');
                if bytes.len() > frame::MAX_RECORD {
                    return Err(Error::Protocol);
                }
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
                if kind == "load" {
                    model_loaded = true;
                }
                if new_signature.is_some() {
                    vad_signature = new_signature;
                }
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
        let cleanup = process.close(grace, close).await;
        // A cancelled snapshot can still be completing private file I/O. Never
        // turn an unconfirmed preparation into a confirmed terminal receipt.
        let settled = match preparation {
            None => true,
            Some(job) if job.is_finished() => job.await.is_ok(),
            Some(_) => false,
        };
        drop(leases);
        Outcome(if settled { cleanup } else { Cleanup::Unknown })
    }
}
