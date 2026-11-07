//! Continuously bounded NUL-delimited inherited CDP input, including idle notifications.
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
};
const LIMIT: u64 = 4 * 1024 * 1024;
type Packet = Result<Value, String>;
type Inbox = mpsc::Receiver<Packet>;
pub(super) fn start<R: AsyncRead + Unpin + Send + 'static>(input: R) -> (Inbox, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<Packet>(8);
    let task = tokio::spawn(async move {
        let mut input = BufReader::new(input);
        loop {
            let mut packet = Vec::new();
            let result = match (&mut input).take(LIMIT).read_until(0, &mut packet).await {
                Ok(0) => Err("CDP connection closed".into()),
                Ok(count) if count >= LIMIT as usize || packet.last() != Some(&0) => {
                    Err("CDP output limit or truncated packet".into())
                }
                Ok(_) => {
                    packet.pop();
                    serde_json::from_slice(&packet).map_err(|_| "Invalid CDP output".into())
                }
                Err(_) => Err("CDP read failed".into()),
            };
            let failed = result.is_err();
            if tx.send(result).await.is_err() || failed {
                break;
            }
        }
    });
    (rx, task)
}
pub(super) struct Abort(pub tokio::task::AbortHandle);
impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    #[tokio::test]
    async fn fragmented_idle_packets_remain_correlated_and_truncation_is_explicit() {
        crate::test_sandbox::assert_isolated();
        let (mut output, input) = tokio::io::duplex(16);
        let (mut incoming, reader) = start(input);
        let writing = tokio::spawn(async move {
            output
                .write_all(b"{\"method\":\"Page.loadEventFired\"}\0{\"id\":")
                .await
                .unwrap();
            tokio::task::yield_now().await;
            output
                .write_all(b"7,\"result\":{}}\0{\"id\":8")
                .await
                .unwrap();
        });
        assert_eq!(
            incoming.recv().await.unwrap().unwrap()["method"],
            "Page.loadEventFired"
        );
        assert_eq!(incoming.recv().await.unwrap().unwrap()["id"], 7);
        assert!(incoming
            .recv()
            .await
            .unwrap()
            .unwrap_err()
            .contains("truncated"));
        writing.await.unwrap();
        reader.await.unwrap();
    }
    #[tokio::test]
    async fn oversized_packets_stop_without_accepting_a_following_frame() {
        crate::test_sandbox::assert_isolated();
        let (mut output, input) = tokio::io::duplex(4096);
        let (mut incoming, reader) = start(input);
        let writing = tokio::spawn(async move {
            let _ = output.write_all(&vec![b'x'; LIMIT as usize]).await;
        });
        assert!(incoming
            .recv()
            .await
            .unwrap()
            .unwrap_err()
            .contains("limit"));
        assert!(incoming.recv().await.is_none());
        reader.await.unwrap();
        writing.await.unwrap();
    }
}
