//! Owned bounded writers keep incoming events and command deadlines independently pollable.
use super::*;
use futures_util::{stream::SplitSink, SinkExt};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
pub(super) struct Writer<T> {
    tx: mpsc::Sender<T>,
    done: tokio::sync::watch::Receiver<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl<T> Writer<T> {
    pub fn send(&self, value: T) -> bool {
        self.tx.try_send(value).is_ok()
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
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        while let Some(value) = rx.recv().await {
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), output.write_all(&value)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        let _ = done_tx.send(true);
    });
    Writer { tx, done, task }
}
pub(super) fn websocket(
    mut output: SplitSink<WebSocketStream<tokio::net::TcpStream>, Message>,
) -> Writer<Message> {
    let (tx, mut rx) = mpsc::channel::<Message>(8);
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        while let Some(value) = rx.recv().await {
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), output.send(value)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        let _ = done_tx.send(true);
    });
    Writer { tx, done, task }
}
