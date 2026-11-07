//! Cancellation-safe bounded NDJSON: partial records remain owned by the reader.
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

pub const MAX_RECORD: usize = 32 * 1024 * 1024;

pub struct Reader<R> {
    input: BufReader<R>,
    partial: Vec<u8>,
}

impl<R: AsyncRead + Unpin> Reader<R> {
    pub fn new(input: R) -> Self {
        Self {
            input: BufReader::new(input),
            partial: Vec::new(),
        }
    }

    pub async fn next(&mut self) -> io::Result<Option<serde_json::Value>> {
        loop {
            let available = self.input.fill_buf().await?;
            if available.is_empty() {
                return if self.partial.is_empty() {
                    Ok(None)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Incomplete speech record",
                    ))
                };
            }
            let end = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|index| index + 1);
            let count = end.unwrap_or(available.len());
            if self
                .partial
                .len()
                .checked_add(count)
                .is_none_or(|length| length > MAX_RECORD)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Speech record exceeds32MiB",
                ));
            }
            self.partial.extend_from_slice(&available[..count]);
            self.input.consume(count);
            if end.is_some() {
                let result = serde_json::from_slice(&self.partial).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid speech record")
                });
                self.partial.clear();
                return result.map(Some);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    #[tokio::test]
    async fn cancelled_partial_read_keeps_exact_next_record() {
        crate::test_sandbox::assert_isolated();
        let (mut write, read) = tokio::io::duplex(64);
        let mut reader = Reader::new(read);
        write.write_all(b"{\"message\":").await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), reader.next())
                .await
                .is_err()
        );
        write.write_all(b"\"retained\"}\n").await.unwrap();
        assert_eq!(reader.next().await.unwrap().unwrap()["message"], "retained");
    }
    #[tokio::test]
    async fn physical_record_bound_includes_lf_and_rejects_before_extension() {
        crate::test_sandbox::assert_isolated();
        for extra in [0, 1] {
            let mut record = vec![b'x'; MAX_RECORD + extra];
            record[0] = b'"';
            let size = record.len();
            record[size - 2] = b'"';
            record[size - 1] = b'\n';
            let (mut write, read) = tokio::io::duplex(64 * 1024);
            let writer = tokio::spawn(async move {
                let _ = write.write_all(&record).await;
            });
            let mut reader = Reader::new(read);
            let result = reader.next().await;
            if extra == 0 {
                assert_eq!(
                    result.unwrap().unwrap().as_str().unwrap().len(),
                    MAX_RECORD - 3
                );
            } else {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
                assert!(reader.partial.len() <= MAX_RECORD);
            }
            drop(reader);
            writer.await.unwrap();
        }
    }
}
