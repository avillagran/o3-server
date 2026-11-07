//! Owned bounded writers keep incoming events and command deadlines independently pollable.
use super::*;
use futures_util::{stream::SplitSink, SinkExt};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
struct Item<T> {
    value: T,
    permit: Option<Permit>,
}
pub(super) trait Authority: Send + Sync {
    fn valid(&self) -> bool;
    /// Linearize a frame start with owner revocation under the same grant lock.
    fn begin(&self) -> bool;
}
#[derive(Clone)]
pub(super) struct Permit {
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    expires: tokio::time::Instant,
    done: tokio::sync::watch::Receiver<bool>,
    control: Option<tokio::sync::watch::Receiver<crate::preview_control::runtime::Snapshot>>,
    authority: Option<std::sync::Arc<dyn Authority>>,
    wake: std::sync::Arc<tokio::sync::Notify>,
}
pub(super) struct CancelOnDrop(Permit);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl Permit {
    pub fn new(
        expires: tokio::time::Instant,
        done: tokio::sync::watch::Receiver<bool>,
        control: tokio::sync::watch::Receiver<crate::preview_control::runtime::Snapshot>,
        wake: std::sync::Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
            expires,
            done,
            control: Some(control),
            authority: None,
            wake,
        }
    }
    pub fn leased(
        expires: tokio::time::Instant,
        done: tokio::sync::watch::Receiver<bool>,
        authority: std::sync::Arc<dyn Authority>,
        wake: std::sync::Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
            expires,
            done,
            control: None,
            authority: Some(authority),
            wake,
        }
    }
    pub fn hold(&self) -> CancelOnDrop {
        CancelOnDrop(self.clone())
    }
    pub fn cancelled(&self) -> bool {
        self.state.load(std::sync::atomic::Ordering::Acquire) >= 2
    }
    /// True means a write may already have started; its outcome remains unknown.
    pub fn cancel(&self) -> bool {
        use std::sync::atomic::Ordering;
        let old = self
            .state
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some(match state {
                    0 => 2,
                    1 => 3,
                    other => other,
                })
            })
            .unwrap();
        self.wake.notify_one();
        old == 1 || old == 3
    }
    fn can_queue(&self) -> bool {
        self.state.load(std::sync::atomic::Ordering::Acquire) == 0
            && tokio::time::Instant::now() < self.expires
            && self.done.has_changed().is_ok()
            && !*self.done.borrow()
            && self
                .control
                .as_ref()
                .is_none_or(|control| control.has_changed().is_ok() && !control.borrow().closed)
            && self
                .authority
                .as_ref()
                .is_none_or(|authority| authority.valid())
    }
    pub fn begin(&self) -> bool {
        if !self.can_queue() {
            self.cancel();
            return false;
        }
        let claimed = self
            .state
            .compare_exchange(
                0,
                1,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if !claimed {
            return false;
        }
        if self
            .authority
            .as_ref()
            .is_some_and(|authority| !authority.begin())
        {
            self.state.store(2, std::sync::atomic::Ordering::Release);
            self.wake.notify_one();
            return false;
        }
        true
    }
}
struct Queue<T> {
    items: std::sync::Mutex<std::collections::VecDeque<Item<T>>>,
    ready: tokio::sync::Notify,
}
impl<T> Queue<T> {
    fn new() -> Self {
        Self {
            items: Default::default(),
            ready: Default::default(),
        }
    }
    fn push(&self, value: T, permit: Option<Permit>) -> bool {
        {
            let mut queue = self.items.lock().unwrap();
            // Reclaim only guarded frames which cannot have begun a write.
            // Legacy FIFO items are never removed or reordered.
            queue.retain(|item| {
                if let Some(permit) = &item.permit {
                    if !permit.can_queue() {
                        permit.cancel();
                        return false;
                    }
                }
                true
            });
            if queue.len() >= 8 {
                return false;
            }
            queue.push_back(Item { value, permit });
        }
        self.ready.notify_one();
        true
    }
    async fn pop(&self) -> Item<T> {
        loop {
            let ready = self.ready.notified();
            let item = self.items.lock().unwrap().pop_front();
            if let Some(item) = item {
                return item;
            }
            ready.await;
        }
    }
}
pub(super) struct Writer<T> {
    queue: std::sync::Arc<Queue<T>>,
    done: tokio::sync::watch::Receiver<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl<T> Writer<T> {
    pub fn send(&self, value: T) -> bool {
        self.queue.push(value, None)
    }
    pub fn send_exact(&self, value: T, permit: Permit) -> bool {
        self.queue.push(value, Some(permit))
    }
    pub fn completion(&self) -> tokio::sync::watch::Receiver<bool> {
        self.done.clone()
    }
    pub async fn stop(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}
impl<T> Drop for Writer<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(super) fn pipe(mut output: tokio::net::unix::OwnedWriteHalf) -> Writer<Vec<u8>> {
    let queue = std::sync::Arc::new(Queue::<Vec<u8>>::new());
    let work = queue.clone();
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        loop {
            let item = work.pop().await;
            if item.permit.as_ref().is_some_and(|permit| !permit.begin()) {
                continue;
            }
            // Once any bytes may have started, finish this frame or use the
            // existing fatal write deadline; canceling partial JSON would
            // corrupt unrelated sessions. The caller retains unknown outcome.
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), output.write_all(&item.value)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        let _ = done_tx.send(true);
    });
    Writer { queue, done, task }
}
pub(super) fn websocket(
    mut output: SplitSink<WebSocketStream<tokio::net::TcpStream>, Message>,
) -> Writer<Message> {
    let queue = std::sync::Arc::new(Queue::<Message>::new());
    let work = queue.clone();
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        loop {
            let item = work.pop().await;
            if item.permit.as_ref().is_some_and(|permit| !permit.begin()) {
                continue;
            }
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), output.send(item.value)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        let _ = done_tx.send(true);
    });
    Writer { queue, done, task }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use tokio::io::AsyncReadExt;
    #[tokio::test]
    async fn queued_expired_or_cancelled_exact_frames_never_write_and_plain_traffic_survives() {
        crate::test_sandbox::assert_isolated();
        let (left, mut right) = tokio::net::UnixStream::pair().unwrap();
        let bytes: libc::c_int = 4096;
        // SAFETY: this is the live owned anonymous fixture socket, and the
        // pointer/length identify the local integer for this synchronous call.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    left.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&bytes as *const libc::c_int).cast(),
                    std::mem::size_of_val(&bytes) as libc::socklen_t,
                )
            },
            0
        );
        let (_input, output) = left.into_split();
        let mut writer = pipe(output);
        let (_done, done) = tokio::sync::watch::channel(false);
        let control = crate::preview_control::runtime::Control::new(None);
        let expired = Permit::new(
            tokio::time::Instant::now() + Duration::from_millis(20),
            done.clone(),
            control.subscribe(),
            Default::default(),
        );
        let cancelled = Permit::new(
            tokio::time::Instant::now() + Duration::from_secs(5),
            done,
            control.subscribe(),
            Default::default(),
        );
        let owner = cancelled.hold();
        assert!(writer.send(vec![b'x'; 512 * 1024]));
        let mut first_byte = [0];
        tokio::time::timeout(Duration::from_secs(2), right.read_exact(&mut first_byte))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&first_byte, b"x");
        assert!(writer.send_exact(vec![b'Z'; 16], expired.clone()));
        for _ in 0..7 {
            assert!(writer.send_exact(vec![b'C'; 16], cancelled.clone()));
        }
        drop(owner);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(writer.send(b"after".to_vec()));
        let mut first = vec![0; 512 * 1024 - 1];
        tokio::time::timeout(Duration::from_secs(2), right.read_exact(&mut first))
            .await
            .unwrap()
            .unwrap();
        assert!(first.iter().all(|byte| *byte == b'x'));
        let mut after = [0; 5];
        tokio::time::timeout(Duration::from_secs(2), right.read_exact(&mut after))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&after, b"after");
        assert!(expired.cancelled() && cancelled.cancelled());
        assert!(!expired.cancel() && !cancelled.cancel());
        writer.stop().await;
        control.close().wait().await.unwrap();
        control.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn stale_admission_is_refused_at_actual_write_without_another_producer() {
        crate::test_sandbox::assert_isolated();
        let (left, mut right) = tokio::net::UnixStream::pair().unwrap();
        let (_input, output) = left.into_split();
        let mut writer = pipe(output);
        let (_done, done) = tokio::sync::watch::channel(false);
        let control = crate::preview_control::runtime::Control::new(None);
        let expired = Permit::new(
            tokio::time::Instant::now() - Duration::from_millis(1),
            done.clone(),
            control.subscribe(),
            Default::default(),
        );
        assert!(writer.send_exact(b"expired".to_vec(), expired));
        assert!(
            tokio::time::timeout(Duration::from_millis(40), right.read_u8())
                .await
                .is_err()
        );
        control.close().wait().await.unwrap();
        let closed = Permit::new(
            tokio::time::Instant::now() + Duration::from_secs(2),
            done,
            control.subscribe(),
            Default::default(),
        );
        assert!(writer.send_exact(b"closed".to_vec(), closed));
        assert!(
            tokio::time::timeout(Duration::from_millis(40), right.read_u8())
                .await
                .is_err()
        );
        assert!(writer.send(b"other".to_vec()));
        let mut after = [0; 5];
        tokio::time::timeout(Duration::from_secs(2), right.read_exact(&mut after))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&after, b"other");
        writer.stop().await;
        control.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn ended_callback_fences_an_unwritten_frame_while_plain_traffic_survives() {
        crate::test_sandbox::assert_isolated();
        let control = crate::preview_control::runtime::Control::new(None);
        let (entered, context) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        let running = control.agent_admitted("agent", None, move |context| async move {
            entered.send(context).unwrap();
            gate.await.unwrap();
            Ok(Value::Null)
        });
        let context = context.await.unwrap();
        let authority = std::sync::Arc::new(exact::CallbackAuthority {
            control: control.clone(),
            context: context.clone(),
        });
        let (_done, done) = tokio::sync::watch::channel(false);
        let permit = Permit::leased(
            tokio::time::Instant::now() + Duration::from_secs(2),
            done,
            authority,
            Default::default(),
        );
        assert!(permit.can_queue());
        release.send(()).unwrap();
        running.wait().await.unwrap();
        assert!(!context.is_active());
        let (left, mut right) = tokio::net::UnixStream::pair().unwrap();
        let (_input, output) = left.into_split();
        let mut writer = pipe(output);
        assert!(writer.send_exact(b"stale-context".to_vec(), permit.clone()));
        assert!(writer.send(b"other".to_vec()));
        let mut received = [0; 5];
        tokio::time::timeout(Duration::from_secs(1), right.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&received, b"other");
        assert!(permit.cancelled());
        writer.stop().await;
        control.shutdown().await.unwrap();
    }
}
