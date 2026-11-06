//! Newline-delimited JSON frames read from provider CLIs (Claude stream-json, Codex app-server).
//!
//! T3 reads these streams with an unbounded line reader. A single native frame can legitimately
//! carry several megabytes: a tool result with base64 screenshots or video frames, a large file
//! read, or aggregated command output. The cap below only bounds transient memory. A frame above
//! it is drained and reported as `Frame::Oversized` so the caller can skip it and keep the turn
//! alive instead of failing the whole run.

use serde_json::Value;
use tokio::io::AsyncBufReadExt;

/// Largest frame that is buffered and parsed.
pub(crate) const MAX_FRAME: usize = 64 * 1024 * 1024;
/// Buffers that grew past this are released after each frame so RSS returns to idle levels.
const RETAINED_CAPACITY: usize = 1024 * 1024;

pub(crate) enum Frame {
    Json(Value, usize),
    /// A frame larger than the limit was discarded; carries its size in bytes.
    Oversized(usize),
}

pub(crate) enum FrameError {
    /// The stream ended in the middle of a frame.
    Truncated,
    Io(String),
    Json(String),
}

/// Cancellation-safe partial-frame state: a cancelled read leaves it consistent for the next call.
#[derive(Default)]
pub(crate) struct FrameBuffer {
    bytes: Vec<u8>,
    discarded: usize,
}

impl FrameBuffer {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    fn release(&mut self) {
        if self.bytes.capacity() > RETAINED_CAPACITY {
            self.bytes = Vec::new();
        } else {
            self.bytes.clear();
        }
    }

    /// Returns `Ok(None)` on a clean end of stream.
    pub(crate) async fn next<R: tokio::io::AsyncBufRead + Unpin>(
        &mut self,
        reader: &mut R,
        limit: usize,
    ) -> Result<Option<Frame>, FrameError> {
        loop {
            let buf = reader
                .fill_buf()
                .await
                .map_err(|e| FrameError::Io(e.to_string()))?;
            if buf.is_empty() {
                return if self.bytes.is_empty() && self.discarded == 0 {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
            let (n, done) = match buf.iter().position(|b| *b == b'\n') {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            };
            if self.discarded > 0 || self.bytes.len() + n > limit {
                self.discarded += self.bytes.len() + n;
                self.release();
                reader.consume(n);
                if done {
                    return Ok(Some(Frame::Oversized(std::mem::take(&mut self.discarded))));
                }
                continue;
            }
            self.bytes.extend_from_slice(&buf[..n]);
            reader.consume(n);
            if done {
                let size = self.bytes.len();
                let parsed = serde_json::from_slice(&self.bytes);
                self.release();
                return parsed
                    .map(|v| Some(Frame::Json(v, size)))
                    .map_err(|e| FrameError::Json(e.to_string()));
            }
        }
    }
}

/// Replaces inline base64 payloads (images, documents) in a native tool result with a size note.
/// Clients never render them from tool output, and persisting them would bloat events and SQLite.
pub(crate) fn redact_inline_binary(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(redact_inline_binary),
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("base64") {
                if let Some(Value::String(data)) = map.get("data") {
                    if data.len() > 1024 {
                        let note = format!("[omitted {} bytes of base64 data]", data.len());
                        map.insert("data".into(), Value::String(note));
                    }
                }
            }
            map.values_mut().for_each(redact_inline_binary);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn oversized_frames_are_skipped_and_reading_continues() {
        let mut input = Vec::new();
        input.extend_from_slice(b"{\"a\":1}\n");
        input.extend(std::iter::repeat_n(b'x', 300));
        input.extend_from_slice(b"\n{\"b\":2}\n");
        let mut reader = BufReader::with_capacity(16, input.as_slice());
        let mut frames = FrameBuffer::default();
        assert!(
            matches!(frames.next(&mut reader, 64).await, Ok(Some(Frame::Json(v, 8))) if v["a"] == 1)
        );
        assert!(matches!(
            frames.next(&mut reader, 64).await,
            Ok(Some(Frame::Oversized(301)))
        ));
        assert!(
            matches!(frames.next(&mut reader, 64).await, Ok(Some(Frame::Json(v, _))) if v["b"] == 2)
        );
        assert!(matches!(frames.next(&mut reader, 64).await, Ok(None)));
    }

    #[tokio::test]
    async fn cancelled_reads_keep_partial_and_discarding_state() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut reader = BufReader::with_capacity(8, reader);
        let mut frames = FrameBuffer::default();
        writer.write_all(&[b'y'; 40]).await.unwrap();
        let pending = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            frames.next(&mut reader, 16),
        );
        assert!(pending.await.is_err());
        writer.write_all(b"yy\n{\"ok\":true}\n").await.unwrap();
        assert!(matches!(
            frames.next(&mut reader, 16).await,
            Ok(Some(Frame::Oversized(43)))
        ));
        assert!(
            matches!(frames.next(&mut reader, 16).await, Ok(Some(Frame::Json(v, _))) if v["ok"] == true)
        );
    }

    #[tokio::test]
    async fn eof_inside_a_frame_is_truncated() {
        let mut reader = BufReader::new(&b"{\"a\":"[..]);
        let mut frames = FrameBuffer::default();
        assert!(matches!(
            frames.next(&mut reader, 64).await,
            Err(FrameError::Truncated)
        ));
    }

    #[test]
    fn inline_base64_is_redacted() {
        let data = "A".repeat(5000);
        let mut block = json!({"type":"tool_result","content":[
            {"type":"text","text":"frame 1"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":data}},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"tiny"}}
        ]});
        redact_inline_binary(&mut block);
        assert_eq!(block["content"][0]["text"], "frame 1");
        assert_eq!(
            block["content"][1]["source"]["data"],
            "[omitted 5000 bytes of base64 data]"
        );
        assert_eq!(block["content"][1]["source"]["media_type"], "image/png");
        assert_eq!(block["content"][2]["source"]["data"], "tiny");
    }
}
