//! Response bodies: small buffered JSON or streamed file bytes behind one type.
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full, StreamBody};
use hyper::body::{Bytes, Frame};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub type Body = UnsyncBoxBody<Bytes, std::io::Error>;

pub fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}
pub fn empty() -> Body {
    full(Bytes::new())
}

/// `len` bytes of `file` from `offset`, read in 64 KiB frames; never more than one frame in memory.
pub fn file_range(file: tokio::fs::File, offset: u64, len: u64) -> Body {
    let stream = futures_util::stream::unfold(
        (file, Some(offset), len),
        |(mut file, seek, left)| async move {
            if left == 0 {
                return None;
            }
            if let Some(offset) = seek.filter(|o| *o > 0) {
                if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
                    return Some((Err(e), (file, None, 0)));
                }
            }
            let mut buf = vec![0u8; left.min(64 * 1024) as usize];
            match file.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((
                        Ok(Frame::data(Bytes::from(buf))),
                        (file, None, left - n as u64),
                    ))
                }
                Err(e) => Some((Err(e), (file, None, 0))),
            }
        },
    );
    StreamBody::new(stream).boxed_unsync()
}
