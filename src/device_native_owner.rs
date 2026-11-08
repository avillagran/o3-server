//! Serialized ownership of one native hub process, independent of transport and installation.
use crate::device_native_client::HubClient;
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::Mutex as AsyncMutex;

pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A handle must stop its owned process when its final handle is dropped.
pub trait HubProcess: Send + Sync {
    fn alive(&self) -> bool;
    fn wait(&self) -> HostFuture<'_, ()>;
    fn stop(&self) -> HostFuture<'_, ()>;
}
#[derive(Clone)]
pub struct SpawnedHub {
    pub process: Arc<dyn HubProcess>,
    pub client: Arc<HubClient>,
    pub origin: String,
}
/// Spawn returns only after readiness; failure/cancellation must close its process scope.
pub trait HubDriver: Send + Sync {
    fn spawn(&self) -> HostFuture<'_, Result<SpawnedHub, Value>>;
}
#[derive(Clone)]
pub struct ReadyHost {
    pub generation: u64,
    pub hub: SpawnedHub,
}
struct Running {
    ready: ReadyHost,
    started: Instant,
}
#[derive(Default)]
struct State {
    running: Option<Running>,
    generation: u64,
    restart_delay: Duration,
    failure: Option<Value>,
}
struct Inner {
    driver: Arc<dyn HubDriver>,
    start_lock: AsyncMutex<()>,
    state: Mutex<State>,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(task) = self.supervisor.get_mut().unwrap().take() {
            task.abort();
        }
    }
}
#[derive(Clone)]
pub struct LocalOwner {
    inner: Arc<Inner>,
}
/// Native crash policy: immediate first replacement; a stable minute resets the sequence.
pub fn restart_delay(current: Duration, uptime: Duration) -> (Duration, Duration) {
    if uptime >= Duration::from_secs(60) {
        return (Duration::ZERO, Duration::ZERO);
    }
    (
        current,
        if current.is_zero() {
            Duration::from_secs(1)
        } else {
            (current * 2).min(Duration::from_secs(30))
        },
    )
}
impl LocalOwner {
    pub fn new(driver: Arc<dyn HubDriver>) -> Self {
        Self {
            inner: Arc::new(Inner {
                driver,
                start_lock: AsyncMutex::new(()),
                state: Mutex::new(State::default()),
                supervisor: Mutex::new(None),
            }),
        }
    }
    pub fn current(&self) -> Option<ReadyHost> {
        self.inner
            .state
            .lock()
            .unwrap()
            .running
            .as_ref()
            .filter(|running| running.ready.hub.process.alive())
            .map(|running| running.ready.clone())
    }
    pub fn failure(&self) -> Option<Value> {
        self.inner.state.lock().unwrap().failure.clone()
    }
    pub async fn ensure_ready(&self) -> Result<ReadyHost, Value> {
        let _lock = self.inner.start_lock.lock().await;
        if let Some(ready) = self.current() {
            return Ok(ready);
        }
        let hub = self.inner.driver.spawn().await?;
        let ready = {
            let mut state = self.inner.state.lock().unwrap();
            state.generation += 1;
            let ready = ReadyHost {
                generation: state.generation,
                hub,
            };
            state.running = Some(Running {
                ready: ready.clone(),
                started: Instant::now(),
            });
            state.restart_delay = Duration::ZERO;
            state.failure = None;
            ready
        };
        Self::supervise(&self.inner, ready.clone());
        Ok(ready)
    }
    fn supervise(inner: &Arc<Inner>, mut ready: ReadyHost) {
        let weak: Weak<Inner> = Arc::downgrade(inner);
        let task = tokio::spawn(async move {
            loop {
                ready.hub.process.wait().await;
                let delay = {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    let mut state = inner.state.lock().unwrap();
                    let Some(running) = state
                        .running
                        .as_ref()
                        .filter(|running| running.ready.generation == ready.generation)
                    else {
                        return;
                    };
                    let uptime = running.started.elapsed();
                    let (delay, next) = restart_delay(state.restart_delay, uptime);
                    state.restart_delay = next;
                    delay
                };
                tokio::time::sleep(delay).await;
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let _lock = inner.start_lock.lock().await;
                if inner
                    .state
                    .lock()
                    .unwrap()
                    .running
                    .as_ref()
                    .is_none_or(|running| running.ready.generation != ready.generation)
                {
                    return;
                }
                let hub = match inner.driver.spawn().await {
                    Ok(hub) => hub,
                    Err(error) => {
                        inner.state.lock().unwrap().failure = Some(error);
                        return;
                    }
                };
                let mut state = inner.state.lock().unwrap();
                state.generation += 1;
                ready = ReadyHost {
                    generation: state.generation,
                    hub,
                };
                state.running = Some(Running {
                    ready: ready.clone(),
                    started: Instant::now(),
                });
                state.failure = None;
            }
        });
        if let Some(old) = inner.supervisor.lock().unwrap().replace(task) {
            old.abort();
        }
    }
    pub async fn stop(&self) {
        let _lock = self.inner.start_lock.lock().await;
        if let Some(task) = self.inner.supervisor.lock().unwrap().take() {
            task.abort();
        }
        let running = {
            let mut state = self.inner.state.lock().unwrap();
            state.generation += 1;
            state.restart_delay = Duration::ZERO;
            state.failure = None;
            state.running.take()
        };
        if let Some(running) = running {
            running.ready.hub.process.stop().await;
        }
    }
}
/// Host errors keep their native structured operation context.
pub fn host_error(host_id: &str, step: &str, cause: impl ToString) -> Value {
    json!({"_tag":"DeviceHostError","hostId":host_id,"step":step,"cause":cause.to_string(),"message":format!("Device host {host_id} failed while {step}.")})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_native_client::HostTools;
    use crate::device_native_hub::{CommandResult, Platform};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Tools;
    impl HostTools for Tools {
        fn platform_available(&self, _: Platform) -> HostFuture<'_, bool> {
            Box::pin(async { true })
        }
        fn run<'a>(&'a self, _: &'a str, _: &'a [String]) -> HostFuture<'a, CommandResult> {
            Box::pin(async {
                CommandResult {
                    code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                }
            })
        }
    }
    struct Process {
        exited: tokio::sync::watch::Sender<bool>,
        stops: AtomicUsize,
    }
    impl HubProcess for Process {
        fn alive(&self) -> bool {
            !*self.exited.borrow()
        }
        fn wait(&self) -> HostFuture<'_, ()> {
            Box::pin(async move {
                let mut rx = self.exited.subscribe();
                while !*rx.borrow_and_update() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            })
        }
        fn stop(&self) -> HostFuture<'_, ()> {
            Box::pin(async move {
                self.stops.fetch_add(1, Ordering::Relaxed);
                self.exited.send_replace(true);
            })
        }
    }
    struct Driver {
        starts: AtomicUsize,
        processes: Mutex<Vec<Arc<Process>>>,
    }
    impl HubDriver for Driver {
        fn spawn(&self) -> HostFuture<'_, Result<SpawnedHub, Value>> {
            Box::pin(async move {
                self.starts.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
                let process = Arc::new(Process {
                    exited: tokio::sync::watch::channel(false).0,
                    stops: AtomicUsize::new(0),
                });
                self.processes.lock().unwrap().push(process.clone());
                Ok(SpawnedHub {
                    process,
                    client: Arc::new(
                        HubClient::new("local".into(), "http://127.0.0.1:1", Arc::new(Tools))
                            .unwrap(),
                    ),
                    origin: "http://127.0.0.1:1".into(),
                })
            })
        }
    }
    #[tokio::test]
    async fn shared_start_owned_restart_and_explicit_stop() {
        crate::test_sandbox::assert_isolated();
        let driver = Arc::new(Driver {
            starts: AtomicUsize::new(0),
            processes: Mutex::new(Vec::new()),
        });
        let owner = LocalOwner::new(driver.clone());
        let (first, second) = tokio::join!(owner.ensure_ready(), owner.ensure_ready());
        assert_eq!(first.unwrap().generation, second.unwrap().generation);
        assert_eq!(driver.starts.load(Ordering::Relaxed), 1);
        driver.processes.lock().unwrap()[0]
            .exited
            .send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), async {
            while driver.starts.load(Ordering::Relaxed) < 2 || owner.current().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(owner.current().unwrap().generation, 2);
        owner.stop().await;
        assert!(owner.current().is_none());
        assert_eq!(
            driver.processes.lock().unwrap()[1]
                .stops
                .load(Ordering::Relaxed),
            1
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(driver.starts.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn native_restart_sequence_and_stable_reset() {
        crate::test_sandbox::assert_isolated();
        let mut current = Duration::ZERO;
        for expected in [0, 1, 2, 4, 8, 16, 30, 30] {
            let (delay, next) = restart_delay(current, Duration::from_secs(1));
            assert_eq!(delay, Duration::from_secs(expected));
            current = next;
        }
        assert_eq!(
            restart_delay(current, Duration::from_secs(60)),
            (Duration::ZERO, Duration::ZERO)
        );
    }
}
