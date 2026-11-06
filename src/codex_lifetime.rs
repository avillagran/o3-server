//! Ownership of inference and transient Codex actors; serialize shared native state initialization.
use super::{Client, Op};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio::sync::{mpsc, oneshot, watch, OwnedMutexGuard};
#[derive(Clone)]
pub(super) struct Actor {
    client: mpsc::WeakSender<Op>,
    finished: watch::Receiver<bool>,
}
impl Actor {
    pub(super) async fn close(mut self) -> Result<(), String> {
        if *self.finished.borrow() {
            return Ok(());
        }
        if let Some(client) = self.client.upgrade() {
            let (ack, _done) = oneshot::channel();
            let _ = client.send(Op::Shutdown(ack)).await;
        }
        while !*self.finished.borrow() {
            self.finished
                .changed()
                .await
                .map_err(|_| "Codex actor ended without a process reap acknowledgement")?;
        }
        Ok(())
    }
}
#[derive(Clone, Default)]
pub(super) struct Actors(Arc<Mutex<Vec<Actor>>>);
impl Actors {
    pub(super) fn track(&self, client: &Client, finished: watch::Receiver<bool>) -> Actor {
        let actor = Actor {
            client: client.0.downgrade(),
            finished,
        };
        let mut actors = self.0.lock().unwrap();
        actors.retain(|a| !*a.finished.borrow());
        actors.push(actor.clone());
        actor
    }
    pub(super) async fn close(&self) -> Result<(), String> {
        let actors = self.0.lock().unwrap().clone();
        for actor in actors {
            actor.close().await?;
        }
        self.0.lock().unwrap().retain(|a| !*a.finished.borrow());
        Ok(())
    }
}
type Gates = Mutex<BTreeMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>;
static STARTUP: OnceLock<Gates> = OnceLock::new();
pub(super) async fn startup(path: PathBuf) -> Startup {
    let gate = {
        let mut gates = STARTUP.get_or_init(Default::default).lock().unwrap();
        gates.retain(|_, v| v.strong_count() > 0);
        if let Some(gate) = gates.get(&path).and_then(Weak::upgrade) {
            gate
        } else {
            let gate = Arc::new(tokio::sync::Mutex::new(()));
            gates.insert(path, Arc::downgrade(&gate));
            gate
        }
    };
    Startup {
        gate: Some(gate.lock_owned().await),
        actor: None,
        runtime: tokio::runtime::Handle::current(),
    }
}
pub(super) struct Startup {
    gate: Option<OwnedMutexGuard<()>>,
    actor: Option<Actor>,
    runtime: tokio::runtime::Handle,
}
impl Startup {
    pub(super) fn spawned(&mut self, actor: Actor) {
        self.actor = Some(actor);
    }
    pub(super) fn initialized(&mut self) {
        self.actor = None;
        self.gate.take();
    }
}
impl Drop for Startup {
    fn drop(&mut self) {
        if let Some(actor) = self.actor.take() {
            let gate = self.gate.take();
            // A cancelled handshake must reap its actor before another process
            // begins SQLite migrations under this home.
            self.runtime.spawn(async move {
                let _ = actor.close().await;
                drop(gate);
            });
        }
    }
}
