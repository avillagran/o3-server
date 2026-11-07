//! Whole-source Silero timeout/backoff policy over the actual owned model store.
use super::{Clock, Task};
use crate::{
    speech_catalog,
    speech_model_store::{Error, Store},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Installation,
    Recording,
}
/// Shared service-scoped retry state. Installation always retries; recording
/// observes the native ten-minute backoff after an unavailable acquisition.
pub struct Silence {
    store: Arc<Store>,
    clock: Arc<dyn Clock>,
    retry_at: Mutex<i64>,
}
impl Silence {
    pub fn new(store: Arc<Store>, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            clock,
            retry_at: Mutex::new(0),
        }
    }
    pub fn acquire(
        &self,
        purpose: Purpose,
        mut cancel: watch::Receiver<bool>,
    ) -> Task<'_, Result<Option<PathBuf>, Error>> {
        Box::pin(async move {
            if *cancel.borrow() {
                return Err(Error::Cancelled);
            }
            if purpose == Purpose::Recording
                && self.clock.now()
                    < *self
                        .retry_at
                        .lock()
                        .map_err(|_| Error::Platform("Silero retry state unavailable".into()))?
            {
                return Ok(None);
            }
            let deadline = self
                .clock
                .now()
                .saturating_add(if purpose == Purpose::Recording {
                    15_000
                } else {
                    120_000
                });
            let result = {
                let progress = |_| {};
                let download =
                    self.store
                        .download(speech_catalog::silero(), &progress, cancel.clone());
                tokio::pin!(download);
                tokio::select! {biased;
                 _=cancel.changed()=>return Err(Error::Cancelled),
                 _=self.clock.sleep_until(deadline)=>None,
                 result=&mut download=>match result {Ok(path)=>Some(path),Err(Error::Cancelled)=>return Err(Error::Cancelled),Err(_)=>None},
                }
            };
            let mut retry = self
                .retry_at
                .lock()
                .map_err(|_| Error::Platform("Silero retry state unavailable".into()))?;
            *retry = if result.is_some() {
                0
            } else {
                self.clock.now().saturating_add(600_000)
            };
            if result.is_none() {
                eprintln!("Silero unavailable; using raw audio with bounded ASR processing.");
            }
            Ok(result)
        })
    }
}
