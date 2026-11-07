//! One scoped native stream. Requests are owned independently of their waiters.
use super::*;
use crate::speech_kernel::{Pcm, MAX_SPEECH_BYTES};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{mpsc, oneshot};

struct Shared {
    commands: mpsc::Sender<Command>,
    busy: AtomicBool,
    stop: watch::Sender<bool>,
    done: watch::Receiver<Option<Result<(), Error>>>,
}
struct Lease(watch::Sender<bool>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
/// A scope owns the reservation until explicit close or its last handle is dropped.
#[derive(Clone)]
pub struct Stream {
    shared: Arc<Shared>,
    _lease: Arc<Lease>,
}
struct Busy(Arc<Shared>);
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}
enum Work {
    Feed(Vec<u8>),
    Finish,
}
struct Command {
    work: Work,
    reply: oneshot::Sender<Result<Value, Error>>,
    _busy: Busy,
}
impl Stream {
    async fn request(&self, work: Work) -> Result<Value, Error> {
        if *self.shared.stop.borrow()
            || self
                .shared
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(not_ready());
        }
        let busy = Busy(self.shared.clone());
        let (reply, result) = oneshot::channel();
        self.shared
            .commands
            .try_send(Command {
                work,
                reply,
                _busy: busy,
            })
            .map_err(|_| not_ready())?;
        result.await.map_err(|_| Error::CleanupUnknown)?
    }
    pub async fn feed(&self, bytes: Vec<u8>) -> Result<Value, Error> {
        self.request(Work::Feed(bytes)).await
    }
    pub async fn finish(&self) -> Result<String, Error> {
        self.request(Work::Finish)
            .await?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(not_ready)
    }
    pub async fn close(&self) -> Result<(), Error> {
        self.shared.stop.send_replace(true);
        let mut done = self.shared.done.clone();
        loop {
            if let Some(result) = done.borrow_and_update().clone() {
                return result;
            }
            if done.changed().await.is_err() {
                return Err(Error::CleanupUnknown);
            }
        }
    }
}
fn not_ready() -> Error {
    operation_error("streaming transcription", "Speech stream is not ready.")
}
struct Opening {
    cancel: watch::Sender<bool>,
    armed: bool,
}
impl Drop for Opening {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.send_replace(true);
        }
    }
}
struct Session {
    inner: Arc<Inner>,
    _permit: Permit,
    model: &'static Model,
    path: std::path::PathBuf,
    settings: Snapshot,
    options: crate::speech_request::TranscriptionOptions,
    loaded: CachedModel,
    stop: watch::Receiver<bool>,
    vad: Option<std::path::PathBuf>,
    received: Vec<Pcm>,
    byte_length: usize,
    retried: bool,
    started: bool,
    finished: bool,
    revision_base: u64,
    last_revision: u64,
}
impl Session {
    async fn begin(&mut self) -> Result<(), Error> {
        if *self.stop.borrow() {
            return Err(Error::Cancelled);
        }
        if let Some(path) = &self.vad {
            self.inner.bind_optional_vad(&self.loaded, path).await?;
        }
        if *self.stop.borrow() {
            return Err(Error::Cancelled);
        }
        self.loaded
            .runtime
            .begin_stream(self.model.id(), &self.options)
            .await?;
        self.started = true;
        Ok(())
    }
    async fn recover(&mut self, cause: Error) -> Result<Option<Value>, Error> {
        self.started = false;
        self.inner.retain_unknown(&cause);
        if cause == Error::CleanupUnknown {
            return Err(cause);
        }
        if *self.stop.borrow()
            || self.retried
            || self.settings.acceleration != "auto"
            || !self.loaded.metadata["backend"]
                .as_str()
                .is_some_and(|backend| !backend.eq_ignore_ascii_case("cpu"))
        {
            return Err(cause);
        }
        self.retried = true;
        self.inner.dispose().await?;
        if *self.stop.borrow() {
            return Err(Error::Cancelled);
        }
        self.loaded = self
            .inner
            .load_cached_at(self.model, "cpu", self.stop.clone(), &self.path)
            .await?;
        self.inner
            .state
            .lock()
            .map_err(|_| Error::CleanupUnknown)?
            .auto_cpu = Some(self.model.id().into());
        self.begin().await?;
        let mut update = None;
        for pcm in &self.received {
            if *self.stop.borrow() {
                return Err(Error::Cancelled);
            }
            update = Some(self.loaded.runtime.feed_stream(pcm).await?);
        }
        Ok(update)
    }
    async fn feed(&mut self, bytes: Vec<u8>) -> Result<Value, Error> {
        if bytes.len() > 64_000 || self.byte_length.saturating_add(bytes.len()) > MAX_SPEECH_BYTES {
            return Err(Error::InvalidAudio {
                byte_length: bytes.len(),
                message: "Speech stream audio limit exceeded.",
            });
        }
        let pcm = Pcm::decode(&bytes).map_err(|cause| Error::InvalidAudio {
            byte_length: cause.byte_length,
            message: cause.message,
        })?;
        self.byte_length += bytes.len();
        self.received.push(pcm);
        let result = self
            .loaded
            .runtime
            .feed_stream(self.received.last().unwrap())
            .await;
        let update = match result {
            Ok(value) => Some(value),
            Err(cause) => {
                let value = self.recover(cause).await?;
                self.revision_base = self.revision_base.saturating_add(self.last_revision);
                self.last_revision = 0;
                value
            }
        };
        if let Some(value) = &update {
            self.last_revision = value["revision"].as_u64().ok_or_else(|| {
                operation_error("streaming transcription", "Invalid speech stream response.")
            })?;
        }
        Ok(
            json!({"revision":self.revision_base+self.last_revision,"text":update.as_ref().map(|value|value["text"].clone()).unwrap_or(Value::Null)}),
        )
    }
    async fn work(&mut self, work: Work) -> Result<Value, Error> {
        if self.finished || !self.started || *self.stop.borrow() {
            return Err(not_ready());
        }
        match work {
            Work::Feed(bytes) => self.feed(bytes).await,
            Work::Finish => {
                let first = self.loaded.runtime.finish_stream().await;
                let result = match first {
                    Ok(text) => Ok(text),
                    Err(cause) => match self.recover(cause).await {
                        Ok(_) => self.loaded.runtime.finish_stream().await,
                        Err(cause) => Err(cause),
                    },
                };
                match result {
                    Ok(text) => {
                        self.finished = true;
                        Ok(json!(text))
                    }
                    Err(cause) => {
                        self.started = false;
                        Err(cause)
                    }
                }
            }
        }
    }
    async fn cleanup(&mut self, deadline: tokio::time::Instant) -> Result<(), Error> {
        if self.finished {
            return Ok(());
        }
        if self.started
            && tokio::time::timeout_at(deadline, self.loaded.runtime.reset_stream())
                .await
                .is_ok_and(|result| result.is_ok())
        {
            return Ok(());
        }
        self.inner.dispose().await
    }
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        done: watch::Sender<Option<Result<(), Error>>>,
    ) {
        let mut stop = self.stop.clone();
        let mut deadline = None;
        loop {
            let command = tokio::select! {biased;
                _=wait_cancelled(&mut stop)=>break,
                command=commands.recv()=>match command {Some(command)=>command,None=>break},
            };
            let Command { work, reply, _busy } = command;
            let mut cancellation = stop.clone();
            let operation = self.work(work);
            tokio::pin!(operation);
            let result = tokio::select! {biased;
                _=wait_cancelled(&mut cancellation)=>{
                    let end=tokio::time::Instant::now()+Duration::from_millis(2500);
                    deadline=Some(end);
                    // Native reset first gives an in-flight request up to one second.
                    let _=tokio::time::timeout(Duration::from_millis(1000),operation.as_mut()).await;
                    None
                },
                result=operation.as_mut()=>Some(result),
            };
            drop(_busy);
            if let Some(result) = result {
                let _ = reply.send(if *stop.borrow() {
                    Err(Error::Cancelled)
                } else {
                    result
                });
            } else {
                let _ = reply.send(Err(Error::Cancelled));
                break;
            }
        }
        let result = self
            .cleanup(
                deadline
                    .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_millis(2500)),
            )
            .await;
        drop(self);
        done.send_replace(Some(result));
    }
}
impl Call {
    pub async fn start_stream(
        &self,
        options: crate::speech_request::TranscriptionOptions,
    ) -> Result<Stream, Error> {
        self.supported()?;
        let inner = self.service.inner.clone();
        let mut permit = inner.reserve("streaming transcription")?;
        let (cancel, stop) = watch::channel(false);
        {
            let mut state = inner.state.lock().map_err(|_| Error::CleanupUnknown)?;
            state.transcribing += 1;
            state.active_cancel = Some(cancel.clone());
            permit.transcribing = true;
        }
        let mut opening = Opening {
            cancel: cancel.clone(),
            armed: true,
        };
        let snapshot = self.snapshot("streaming transcription").await?;
        let model = speech_catalog::selected(&snapshot.model_id);
        let result = owned(async move {
            let path = inner.installed_model_path(model).await?;
            let loaded = inner
                .load_cached_at(model, &snapshot.acceleration, stop.clone(), &path)
                .await?;
            let mut session = Session {
                inner: inner.clone(),
                _permit: permit,
                model,
                path,
                settings: snapshot,
                options,
                loaded,
                stop,
                vad: None,
                received: vec![],
                byte_length: 0,
                retried: false,
                started: false,
                finished: false,
                revision_base: 0,
                last_revision: 0,
            };
            let needs_vad = session.options.as_value()["speechSilenceFiltering"] == true
                || !(session.loaded.metadata["supportsStreaming"] == true
                    || session.loaded.metadata["supportsLongForm"] == true);
            let setup = async {
                if needs_vad {
                    if let Some(path) = inner
                        .silence
                        .acquire(silence::Purpose::Recording, session.stop.clone())
                        .await
                        .map_err(|cause| operation_error("streaming transcription", cause))?
                    {
                        session.vad = Some(path);
                    }
                }
                if let Err(cause) = session.begin().await {
                    session.recover(cause).await?;
                }
                Ok::<_, Error>(())
            }
            .await;
            if let Err(cause) = setup {
                inner.dispose().await?;
                return Err(cause);
            }
            let (commands, receiver) = mpsc::channel(1);
            let (done, completion) = watch::channel(None);
            let shared = Arc::new(Shared {
                commands,
                busy: AtomicBool::new(false),
                stop: cancel.clone(),
                done: completion,
            });
            let stream = Stream {
                shared,
                _lease: Arc::new(Lease(cancel)),
            };
            tokio::spawn(session.run(receiver, done));
            Ok(stream)
        })
        .await;
        opening.armed = false;
        result
    }
}
