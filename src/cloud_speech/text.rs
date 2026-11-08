//! One owned text actor; no ASR package/model, arbitrary module, or replay authority.
use super::*;
use crate::speech_worker::{Cleanup, NodeRuntime, TextWorkerSpec, Worker};
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};
enum Work {
    Prepare,
    Cloud {
        value: String,
        options: Value,
        language: String,
    },
    Prompt {
        instructions: String,
        transcript: String,
        draft: Value,
        correction: String,
    },
    Postprocess {
        value: String,
        original: String,
        words: Value,
    },
}
enum Output {
    Prepared,
    Text(String),
    Units(Vec<u16>),
}
struct Command {
    work: Work,
    cancel: watch::Sender<bool>,
    reply: oneshot::Sender<Result<Output, Error>>,
}
struct Caller {
    cancel: watch::Sender<bool>,
    armed: bool,
}
impl Drop for Caller {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.send_replace(true);
        }
    }
}
pub struct NativeText {
    commands: mpsc::Sender<Command>,
    stop: watch::Sender<bool>,
    done: watch::Receiver<Option<Result<(), Error>>>,
}
impl NativeText {
    pub fn new(node: Arc<NodeRuntime>, directory: PathBuf) -> Result<Self, Error> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::Settings)?;
        let (commands, receiver) = mpsc::channel(1);
        let (stop, stopped) = watch::channel(false);
        let (done, completion) = watch::channel(None);
        runtime.spawn(async move {
            let result = Actor {
                node,
                directory,
                commands: receiver,
                stop: stopped,
                worker: None,
                unknown: false,
            }
            .run()
            .await;
            done.send_replace(Some(result));
        });
        Ok(Self {
            commands,
            stop,
            done: completion,
        })
    }
    async fn request(&self, work: Work) -> Result<Output, Error> {
        if *self.stop.borrow() {
            return Err(Error::Cancelled);
        }
        let (cancel, _) = watch::channel(false);
        let mut caller = Caller {
            cancel: cancel.clone(),
            armed: true,
        };
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command {
                work,
                cancel,
                reply,
            })
            .await
            .map_err(|_| Error::Cancelled)?;
        let result = receiver.await.map_err(|_| Error::CleanupUnknown)?;
        caller.armed = false;
        result
    }
    pub async fn prompt(
        &self,
        instructions: &str,
        transcript: &str,
        draft: &Value,
        correction: &str,
    ) -> Result<Vec<u16>, Error> {
        match self
            .request(Work::Prompt {
                instructions: instructions.into(),
                transcript: transcript.into(),
                draft: draft.clone(),
                correction: correction.into(),
            })
            .await?
        {
            Output::Units(value) => Ok(value),
            _ => Err(Error::CleanupUnknown),
        }
    }
    pub async fn postprocess(
        &self,
        value: &str,
        original: &str,
        words: &Value,
    ) -> Result<String, Error> {
        match self
            .request(Work::Postprocess {
                value: value.into(),
                original: original.into(),
                words: words.clone(),
            })
            .await?
        {
            Output::Text(value) => Ok(value),
            _ => Err(Error::CleanupUnknown),
        }
    }
}
impl Drop for NativeText {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}
impl Text for NativeText {
    fn prepare(&self) -> Task<'_, Result<(), Error>> {
        Box::pin(async move {
            match self.request(Work::Prepare).await? {
                Output::Prepared => Ok(()),
                _ => Err(Error::CleanupUnknown),
            }
        })
    }
    fn cloud<'a>(
        &'a self,
        value: &'a str,
        options: &'a TranscriptionOptions,
        language: &'a str,
    ) -> Task<'a, Result<String, Error>> {
        Box::pin(async move {
            match self
                .request(Work::Cloud {
                    value: value.into(),
                    options: options.as_value().clone(),
                    language: language.into(),
                })
                .await?
            {
                Output::Text(value) => Ok(value),
                _ => Err(Error::CleanupUnknown),
            }
        })
    }
    fn shutdown(&self) -> Task<'_, Result<(), Error>> {
        self.stop.send_replace(true);
        Box::pin(async move {
            let mut done = self.done.clone();
            loop {
                if let Some(result) = done.borrow_and_update().clone() {
                    return result;
                }
                if done.changed().await.is_err() {
                    return Err(Error::CleanupUnknown);
                }
            }
        })
    }
}
struct Actor {
    node: Arc<NodeRuntime>,
    directory: PathBuf,
    commands: mpsc::Receiver<Command>,
    stop: watch::Receiver<bool>,
    worker: Option<Worker>,
    unknown: bool,
}
impl Actor {
    async fn dispose(&mut self) -> Result<(), Error> {
        if let Some(worker) = self.worker.take() {
            if worker.close().await == Cleanup::Unknown {
                self.unknown = true;
            }
        }
        if self.unknown {
            Err(Error::CleanupUnknown)
        } else {
            Ok(())
        }
    }
    async fn start(&mut self, cancel: &watch::Sender<bool>) -> Result<(), Error> {
        if self.unknown {
            return Err(Error::CleanupUnknown);
        }
        if *self.stop.borrow() || *cancel.borrow() {
            return Err(Error::Cancelled);
        }
        if self.worker.is_some() {
            return Ok(());
        }
        let spec = TextWorkerSpec {
            node: self.node.clone(),
            directory: self
                .directory
                .join(crate::store::random_id().map_err(|_| Error::Settings)?),
            startup_deadline: Duration::from_secs(10),
            operation_deadline: Duration::from_secs(120),
            close_deadline: Duration::from_secs(5),
        };
        let startup = Worker::start_text(spec, cancel.subscribe());
        tokio::pin!(startup);
        // The actor owns this future after its caller disappears. Global shutdown
        // signals the same admission receiver and still awaits the startup receipt.
        let result = tokio::select! {biased;_=cancelled(&mut self.stop)=>{cancel.send_replace(true);startup.await},result=startup.as_mut()=>result};
        match result {
            Ok(worker) => self.worker = Some(worker),
            Err(cause) => {
                if cause
                    .get_ref()
                    .and_then(|cause| cause.downcast_ref::<crate::speech_worker::StartupFailure>())
                    .is_some_and(|failure| failure.cleanup == Cleanup::Unknown)
                {
                    self.unknown = true;
                    return Err(Error::CleanupUnknown);
                }
                return Err(Error::Cancelled);
            }
        }
        if *self.stop.borrow() || *cancel.borrow() {
            self.dispose().await?;
            return Err(Error::Cancelled);
        }
        Ok(())
    }
    async fn perform(worker: &Worker, work: Work) -> Result<Output, crate::speech_worker::Error> {
        Ok(match work {
            Work::Prepare => {
                worker.text_ready().await?;
                Output::Prepared
            }
            Work::Cloud {
                value,
                options,
                language,
            } => {
                let options = TranscriptionOptions::decode(options)
                    .map_err(|_| crate::speech_worker::Error::InvalidInput)?;
                Output::Text(worker.correct_cloud(&value, &options, &language).await?)
            }
            Work::Prompt {
                instructions,
                transcript,
                draft,
                correction,
            } => Output::Units(
                worker
                    .postprocess_prompt(&instructions, &transcript, &draft, &correction)
                    .await?,
            ),
            Work::Postprocess {
                value,
                original,
                words,
            } => Output::Text(
                worker
                    .correct_postprocess(&value, &original, &words)
                    .await?,
            ),
        })
    }
    async fn command(&mut self, work: Work, cancel: &watch::Sender<bool>) -> Result<Output, Error> {
        self.start(cancel).await?;
        let mut cancellation = cancel.subscribe();
        let result = tokio::select! {biased;_=cancelled(&mut self.stop)=>None,_=cancelled(&mut cancellation)=>None,result=Self::perform(self.worker.as_ref().unwrap(),work)=>Some(result)};
        if matches!(
            &result,
            Some(Err(crate::speech_worker::Error::CleanupUnknown))
        ) {
            self.unknown = true;
        }
        match result {
            Some(Ok(value)) if !*self.stop.borrow() && !*cancel.borrow() => Ok(value),
            _ => {
                self.dispose().await?;
                Err(Error::Cancelled)
            }
        }
    }
    async fn run(mut self) -> Result<(), Error> {
        loop {
            let command = tokio::select! {biased;_=cancelled(&mut self.stop)=>break,command=self.commands.recv()=>match command{Some(command)=>command,None=>break}};
            let result = self.command(command.work, &command.cancel).await;
            if command.reply.send(result).is_err() {
                self.dispose().await?;
            }
        }
        self.dispose().await
    }
}
