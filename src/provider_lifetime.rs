//! Native instance scopes, following nightly4179 ProviderInstanceRegistry (MIT).
use crate::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::Value;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{watch, RwLock};
type Result<T> = std::result::Result<T, String>;
type CloseReceiver = watch::Receiver<Option<Result<()>>>;
pub(crate) type Scopes = std::collections::BTreeMap<String, Arc<Pool>>;
pub(crate) struct Pool {
    provider: Arc<dyn Provider>,
    retired: watch::Sender<bool>,
    closed: Arc<AtomicBool>,
    operations: Arc<RwLock<()>>,
    closing: Mutex<Option<CloseReceiver>>,
    predecessors: Mutex<Vec<Arc<Pool>>>,
}
impl Pool {
    pub(crate) fn new(provider: Arc<dyn Provider>, predecessors: Vec<Arc<Pool>>) -> Arc<Self> {
        let (retired, _) = watch::channel(false);
        Arc::new(Self {
            provider,
            retired,
            closed: Arc::new(AtomicBool::new(false)),
            operations: Arc::new(RwLock::new(())),
            closing: Mutex::new(None),
            predecessors: Mutex::new(predecessors),
        })
    }
    pub(crate) fn retire(&self) {
        self.retired.send_replace(true);
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    pub(crate) async fn close(&self) -> Result<()> {
        self.retire();
        // A caller may disappear while shutdown is reaping children. The worker
        // owns cleanup independently; every replacement waits for its acknowledgement.
        let mut done = {
            let mut closing = self.closing.lock().unwrap();
            if self.is_closed() {
                return Ok(());
            }
            if let Some(done) = closing
                .as_ref()
                .filter(|done| done.borrow().is_none() && done.has_changed().is_ok())
            {
                done.clone()
            } else {
                let (result, done) = watch::channel(None);
                let (provider, operations, closed) = (
                    self.provider.clone(),
                    self.operations.clone(),
                    self.closed.clone(),
                );
                tokio::spawn(async move {
                    let _guard = operations.write().await;
                    let outcome = provider.dispose().await;
                    if outcome.is_ok() {
                        closed.store(true, Ordering::Release);
                    }
                    result.send_replace(Some(outcome));
                });
                *closing = Some(done.clone());
                done
            }
        };
        loop {
            if let Some(result) = done.borrow().clone() {
                return result;
            }
            done.changed()
                .await
                .map_err(|_| "Provider instance close ended without acknowledgement")?;
        }
    }
    async fn ready(&self) -> Result<()> {
        let previous = self.predecessors.lock().unwrap().clone();
        for pool in previous {
            pool.close().await?;
        }
        self.predecessors.lock().unwrap().retain(|p| !p.is_closed());
        if *self.retired.borrow() {
            return Err(
                "Provider instance settings changed; the native session was retired.".into(),
            );
        }
        Ok(())
    }
    async fn operation<T>(&self, work: impl Future<Output = Result<T>>) -> Result<T> {
        self.ready().await?;
        let _guard = self.operations.read().await;
        let mut retired = self.retired.subscribe();
        if *retired.borrow() {
            return Err(
                "Provider instance settings changed; the native session was retired.".into(),
            );
        }
        tokio::select! {biased;
            _=retired.changed()=>Err("Provider instance settings changed; the native session was retired.".into()),
            result=work=>result,
        }
    }
}
impl Provider for Pool {
    fn capabilities(&self, instance: &str) -> Option<Value> {
        self.provider.capabilities(instance)
    }
    fn plan_selection_transition(
        &self,
        instance: &str,
        current: &Value,
        target: &Value,
        capabilities: &Value,
    ) -> Result<Option<crate::selection_transition::Selection>> {
        self.provider
            .plan_selection_transition(instance, current, target, capabilities)
    }
    fn config(&self) -> Vec<Value> {
        self.provider.config()
    }
    fn dispose<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(self.close())
    }
    fn refresh<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(self.operation(self.provider.refresh()))
    }
    fn discover<'a>(
        &'a self,
        cwd: std::path::PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(self.operation(self.provider.discover(cwd)))
    }
    fn poll_usage<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(self.operation(self.provider.poll_usage()))
    }
    fn detach<'a>(
        &'a self,
        instance: &'a str,
        key: &'a str,
        native: Option<&'a str>,
        revoke: bool,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if self.is_closed() {
                return Ok(());
            }
            self.operation(self.provider.detach(instance, key, native, revoke))
                .await
        })
    }
    fn execute<'a>(
        &'a self,
        input: RunInput,
        output: tokio::sync::mpsc::Sender<ProviderEvent>,
        mut control: tokio::sync::mpsc::Receiver<ProviderControl>,
        cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.ready().await?;
            let _guard = self.operations.read().await;
            let mut retired = self.retired.subscribe();
            if *retired.borrow() {
                return Err(
                    "Provider instance settings changed; the native session was retired.".into(),
                );
            }
            let owner = input.native_mcp.clone();
            let (forward, controls) = tokio::sync::mpsc::channel(8);
            let mut forward = Some(forward);
            let work = self.provider.execute(input, output, controls, cancel);
            tokio::pin!(work);
            let mut retiring = false;
            let mut control_open = true;
            loop {
                tokio::select! {biased;
                    _=retired.changed(),if !retiring=>{
                        retiring=true;
                        if let Some(owner)=&owner {owner.revoke();}
                        if let Some(forward)=&forward {let _=forward.send(ProviderControl::Interrupt).await;}
                    },
                    result=&mut work=>return if retiring {result.and(Err("Provider instance settings changed; the native session was retired.".into()))}else{result},
                    action=control.recv(),if control_open&&!retiring=>match action {
                        Some(action)=>{if let Some(forward)=&forward {let _=forward.send(action).await;}},
                        None=>{control_open=false;forward.take();},
                    },
                }
            }
        })
    }
}

/// A builtin in a legacy router has the same lifetime boundary as an explicit instance.
pub(crate) struct Builtin {
    router: Arc<dyn Provider>,
    id: String,
}
impl Builtin {
    pub(crate) fn new(router: Arc<dyn Provider>, id: String) -> Self {
        Self { router, id }
    }
    fn excluded(&self) -> Vec<String> {
        self.router
            .config()
            .into_iter()
            .filter_map(|v| {
                v["instanceId"]
                    .as_str()
                    .filter(|id| *id != self.id)
                    .map(str::to_owned)
            })
            .collect()
    }
}
impl Provider for Builtin {
    fn capabilities(&self, instance: &str) -> Option<Value> {
        self.router.capabilities(instance)
    }
    fn plan_selection_transition(
        &self,
        instance: &str,
        current: &Value,
        target: &Value,
        caps: &Value,
    ) -> Result<Option<crate::selection_transition::Selection>> {
        self.router
            .plan_selection_transition(instance, current, target, caps)
    }
    fn config(&self) -> Vec<Value> {
        self.router
            .config()
            .into_iter()
            .filter(|v| v["instanceId"] == self.id)
            .collect()
    }
    fn dispose<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        self.router.dispose_instance(&self.id)
    }
    fn refresh<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        self.router.refresh_defaults(self.excluded())
    }
    fn discover<'a>(
        &'a self,
        cwd: std::path::PathBuf,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        self.router.discover_defaults(cwd, self.excluded())
    }
    fn poll_usage<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        self.router.poll_defaults(self.excluded())
    }
    fn detach<'a>(
        &'a self,
        instance: &'a str,
        key: &'a str,
        native: Option<&'a str>,
        revoke: bool,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        self.router.detach(instance, key, native, revoke)
    }
    fn execute<'a>(
        &'a self,
        input: RunInput,
        output: tokio::sync::mpsc::Sender<ProviderEvent>,
        control: tokio::sync::mpsc::Receiver<ProviderControl>,
        cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        self.router.execute(input, output, control, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::{Notify, Semaphore};
    struct Peer {
        opens: AtomicUsize,
        closes: AtomicUsize,
        entered: Notify,
        closing: Notify,
        work: Semaphore,
        cleanup: Semaphore,
        failed: AtomicBool,
        dropped: Arc<AtomicBool>,
    }
    struct Work(Arc<AtomicBool>);
    impl Drop for Work {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    impl Peer {
        fn new(hold_work: bool, hold_close: bool) -> Arc<Self> {
            Arc::new(Self {
                opens: AtomicUsize::new(0),
                closes: AtomicUsize::new(0),
                entered: Notify::new(),
                closing: Notify::new(),
                work: Semaphore::new(usize::from(!hold_work)),
                cleanup: Semaphore::new(usize::from(!hold_close)),
                failed: AtomicBool::new(false),
                dropped: Arc::new(AtomicBool::new(false)),
            })
        }
    }
    impl Provider for Peer {
        fn refresh<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                self.opens.fetch_add(1, Ordering::AcqRel);
                let _work = Work(self.dropped.clone());
                self.entered.notify_one();
                let _guard = self.work.acquire().await.unwrap();
                Ok(())
            })
        }
        fn dispose<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                assert!(
                    self.opens.load(Ordering::Acquire) == 0 || self.dropped.load(Ordering::Acquire),
                    "shutdown raced an admitted native operation"
                );
                self.closing.notify_one();
                let _guard = self.cleanup.acquire().await.unwrap();
                if self.failed.load(Ordering::Acquire) {
                    return Err("cleanup failed".into());
                }
                self.closes.fetch_add(1, Ordering::AcqRel);
                Ok(())
            })
        }
        fn execute<'a>(
            &'a self,
            _: RunInput,
            _: tokio::sync::mpsc::Sender<ProviderEvent>,
            _: tokio::sync::mpsc::Receiver<ProviderControl>,
            _: watch::Receiver<bool>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            self.refresh()
        }
    }
    #[tokio::test]
    async fn replacement_cancels_old_work_and_waits_for_physical_close_without_blocking_peers() {
        let a = Peer::new(true, true);
        let old = Pool::new(a.clone(), vec![]);
        let started = old.clone();
        let work = tokio::spawn(async move { started.refresh().await });
        a.entered.notified().await;
        old.retire();
        let b = Peer::new(false, false);
        let next = Pool::new(b.clone(), vec![old.clone()]);
        let start = next.clone();
        let replacement = tokio::spawn(async move { start.refresh().await });
        a.closing.notified().await;
        assert!(work.await.unwrap().unwrap_err().contains("retired"));
        assert_eq!(b.opens.load(Ordering::Acquire), 0);
        let peer = Peer::new(false, false);
        Pool::new(peer.clone(), vec![]).refresh().await.unwrap();
        assert_eq!(peer.opens.load(Ordering::Acquire), 1);
        a.cleanup.add_permits(1);
        replacement.await.unwrap().unwrap();
        assert_eq!(a.closes.load(Ordering::Acquire), 1);
        assert!(old.refresh().await.is_err());
        assert_eq!(a.opens.load(Ordering::Acquire), 1);
        old.close().await.unwrap();
        assert_eq!(a.closes.load(Ordering::Acquire), 1);
    }
    #[tokio::test]
    async fn abandoning_a_waiter_does_not_abandon_cleanup_or_admit_a_replacement() {
        let a = Peer::new(false, true);
        let old = Pool::new(a.clone(), vec![]);
        old.retire();
        let retiring = old.clone();
        let waiter = tokio::spawn(async move { retiring.close().await });
        a.closing.notified().await;
        waiter.abort();
        let _ = waiter.await;
        let b = Peer::new(false, false);
        let next = Pool::new(b.clone(), vec![old]);
        let waiting = tokio::spawn(async move { next.refresh().await });
        tokio::task::yield_now().await;
        assert_eq!(b.opens.load(Ordering::Acquire), 0);
        a.cleanup.add_permits(1);
        waiting.await.unwrap().unwrap();
        assert_eq!(a.closes.load(Ordering::Acquire), 1);
        assert_eq!(b.opens.load(Ordering::Acquire), 1);
    }
    #[tokio::test]
    async fn failed_close_refuses_replacement_and_a_later_call_retries_cleanup() {
        let a = Peer::new(false, false);
        a.failed.store(true, Ordering::Release);
        let old = Pool::new(a.clone(), vec![]);
        old.retire();
        let b = Peer::new(false, false);
        let next = Pool::new(b.clone(), vec![old.clone()]);
        assert_eq!(next.refresh().await.unwrap_err(), "cleanup failed");
        assert!(!old.is_closed());
        assert_eq!(b.opens.load(Ordering::Acquire), 0);
        a.failed.store(false, Ordering::Release);
        next.refresh().await.unwrap();
        assert_eq!(a.closes.load(Ordering::Acquire), 1);
        assert_eq!(b.opens.load(Ordering::Acquire), 1);
    }
    #[test]
    fn synchronous_metadata_retirement_still_closes_before_native_start() {
        let a = Peer::new(false, false);
        let old = Pool::new(a.clone(), vec![]);
        old.retire();
        let b = Peer::new(false, false);
        let next = Pool::new(b.clone(), vec![old]);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                next.refresh().await.unwrap();
            });
        assert_eq!(a.closes.load(Ordering::Acquire), 1);
        assert_eq!(b.opens.load(Ordering::Acquire), 1);
    }
}
