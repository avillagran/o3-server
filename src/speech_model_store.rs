//! Source-pinned ModelStore algorithm. Filesystem ownership is an explicit backend.
mod local;
use crate::speech_catalog::Model;
pub use local::{Http, Local};
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

pub type Task<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Status(u16),
    Range,
    Size,
    Verification,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Download { model_id: String, failure: Failure },
    Platform(String),
    Transport(String),
    Cancelled,
}
impl Error {
    fn download(model: &Model, failure: Failure) -> Self {
        Self::Download {
            model_id: model.id().to_owned(),
            failure,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Download { failure, .. } => {
                let reason = match failure {
                    Failure::Status(_) => "status",
                    Failure::Range => "range",
                    Failure::Size => "size",
                    Failure::Verification => "verification",
                };
                write!(f, "Speech model download failed: {reason}")?;
                if let Failure::Status(status) = failure {
                    write!(f, " {status}")?;
                }
                f.write_str(".")
            }
            Self::Platform(message) | Self::Transport(message) => f.write_str(message),
            Self::Cancelled => f.write_str("Speech model download cancelled."),
        }
    }
}
impl std::error::Error for Error {}

/// Native identity intentionally uses millisecond mtime, not ctime. Execution
/// admission independently hashes model bytes before the worker receives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub regular: bool,
    pub size: u64,
    pub mtime_ms: Option<i64>,
    pub device: u64,
    pub inode: Option<u64>,
}
impl FileInfo {
    pub fn marker(&self, model: &Model) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            model.sha256(),
            self.size,
            self.mtime_ms.map(|n| n.to_string()).unwrap_or_default(),
            self.device,
            self.inode.map(|n| n.to_string()).unwrap_or_default()
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Downloaded(u64),
    VerificationStarted,
}
pub trait Body: Send {
    fn next(&mut self) -> Task<'_, Result<Option<Vec<u8>>, Error>>;
}
pub struct Response {
    pub status: u16,
    pub content_range: Option<String>,
    pub content_length: Option<String>,
    pub body: Box<dyn Body>,
}
pub trait Transport: Send + Sync {
    fn get<'a>(&'a self, url: &'a str, offset: u64) -> Task<'a, Result<Response, Error>>;
}
/// A transfer owns its unique staging and retention receipt. Drop must fence
/// publication and retain/quarantine it; it must not purge directory trees.
pub trait Transfer: Send {
    fn path(&self) -> &Path;
    fn offset(&self) -> u64;
    fn restart(&mut self) -> Task<'_, Result<(), Error>>;
    fn append<'a>(&'a mut self, bytes: &'a [u8]) -> Task<'a, Result<(), Error>>;
    fn publish<'a>(&'a mut self, final_path: &'a Path) -> Task<'a, Result<(), Error>>;
}
pub trait Backend: Send + Sync {
    fn directory(&self) -> &Path;
    fn info(&self, path: &Path) -> Option<FileInfo>;
    fn read_marker(&self, path: &Path) -> Option<String>;
    fn write_marker(&self, path: &Path, marker: &str) -> Result<(), Error>;
    fn digest<'a>(&'a self, path: &'a Path) -> Task<'a, Option<String>>;
    fn stage<'a>(&'a self, model: &'a Model) -> Task<'a, Result<Box<dyn Transfer>, Error>>;
    /// The production implementation uses an exact-file identity plan; tests
    /// can explicitly record that plan without physically removing fixtures.
    fn remove<'a>(&'a self, model: &'a Model) -> Task<'a, Result<(), Error>>;
}
pub struct Store {
    backend: Arc<dyn Backend>,
    transport: Arc<dyn Transport>,
    corrupt: Mutex<HashMap<PathBuf, String>>,
}
impl Store {
    pub fn new(backend: Arc<dyn Backend>, transport: Arc<dyn Transport>) -> Self {
        Self {
            backend,
            transport,
            corrupt: Mutex::new(HashMap::new()),
        }
    }
    pub fn path(&self, model: &Model) -> PathBuf {
        self.backend.directory().join(model.filename())
    }
    fn expected(&self, model: &Model, path: &Path) -> Option<FileInfo> {
        self.backend
            .info(path)
            .filter(|info| info.regular && info.size == model.size())
    }
    pub fn is_ready(&self, model: &Model) -> bool {
        let path = self.path(model);
        self.expected(model, &path).is_some_and(|info| {
            self.corrupt
                .lock()
                .map(|bad| bad.get(&path) != Some(&info.marker(model)))
                .unwrap_or(false)
        })
    }
    async fn verified(
        &self,
        model: &Model,
        path: &Path,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Option<PathBuf> {
        let info = self.expected(model, path)?;
        let marker = info.marker(model);
        if self.corrupt.lock().ok()?.get(path) == Some(&marker) {
            return None;
        }
        if self.backend.read_marker(path).as_deref() == Some(&marker) {
            return Some(path.to_owned());
        }
        progress(Progress::VerificationStarted);
        let valid = self.backend.digest(path).await.as_deref() == Some(model.sha256());
        if !valid {
            self.corrupt
                .lock()
                .ok()?
                .insert(path.to_owned(), marker.clone());
        }
        let current = self.expected(model, path)?;
        if !valid || current.marker(model) != marker {
            return None;
        }
        let _ = self.backend.write_marker(path, &marker);
        Some(path.to_owned())
    }
    pub async fn installed_path(&self, model: &Model) -> Option<PathBuf> {
        self.verified(model, &self.path(model), &|_| {}).await
    }
    pub async fn remove(&self, model: &Model) -> Result<(), Error> {
        self.backend.remove(model).await
    }
    pub async fn download(
        &self,
        model: &Model,
        progress: &(dyn Fn(Progress) + Send + Sync),
        mut cancel: watch::Receiver<bool>,
    ) -> Result<PathBuf, Error> {
        let operation = async {
            let path = self.path(model);
            if let Some(installed) = self.verified(model, &path, progress).await {
                return Ok(installed);
            }
            let mut transfer = self.backend.stage(model).await?;
            let url = model.url();
            for attempt in 1..=3 {
                let offset = transfer.offset();
                if offset == model.size() {
                    break;
                }
                let fetched = self
                    .fetch(model, &url, transfer.as_mut(), offset, progress)
                    .await;
                match fetched {
                    Ok(downloaded) if downloaded == model.size() => break,
                    Ok(_) => {}
                    Err(error)
                        if attempt < 3
                            && !matches!(error, Error::Download { .. } | Error::Cancelled) => {}
                    Err(error) => return Err(error),
                }
            }
            progress(Progress::VerificationStarted);
            if self.expected(model, transfer.path()).is_none()
                || self.backend.digest(transfer.path()).await.as_deref() != Some(model.sha256())
            {
                return Err(Error::download(model, Failure::Verification));
            }
            transfer.publish(&path).await?;
            // Match the native post-rename stat and best-effort marker write.
            let info = self
                .backend
                .info(&path)
                .ok_or_else(|| Error::Platform("Installed model stat failed".into()))?;
            let _ = self.backend.write_marker(&path, &info.marker(model));
            Ok(path)
        };
        if *cancel.borrow() {
            return Err(Error::Cancelled);
        }
        tokio::select! {biased;_=cancel.changed()=>Err(Error::Cancelled),result=operation=>result}
    }
    async fn fetch(
        &self,
        model: &Model,
        url: &str,
        transfer: &mut dyn Transfer,
        offset: u64,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<u64, Error> {
        let mut response = self.transport.get(url, offset).await?;
        let resumed = offset > 0 && response.status == 206;
        if !resumed && response.status != 200 {
            return Err(Error::download(model, Failure::Status(response.status)));
        }
        if resumed
            && !valid_range(
                response.content_range.as_deref().unwrap_or(""),
                offset,
                model.size(),
            )
        {
            return Err(Error::download(model, Failure::Range));
        }
        let start = if resumed { offset } else { 0 };
        if let Some(length) = response.content_length.as_deref() {
            let number = js_number(length);
            if number.is_finite() && number != (model.size() as f64 - start as f64) {
                return Err(Error::download(model, Failure::Size));
            }
        }
        let mut downloaded = start;
        progress(Progress::Downloaded(downloaded));
        if !resumed {
            transfer.restart().await?;
        }
        while let Some(chunk) = response.body.next().await? {
            downloaded = downloaded
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| Error::download(model, Failure::Size))?;
            if downloaded > model.size() {
                return Err(Error::download(model, Failure::Size));
            }
            progress(Progress::Downloaded(downloaded));
            transfer.append(&chunk).await?;
        }
        Ok(downloaded)
    }
}
fn valid_range(value: &str, offset: u64, size: u64) -> bool {
    static RANGE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let regex = RANGE.get_or_init(|| {
        regex::Regex::new(r"^bytes ([0-9]+)-([0-9]+)/([0-9]+)$")
            .expect("Static native range pattern")
    });
    regex.captures(value).is_some_and(|parts| {
        parts[1].parse::<u64>().ok() == Some(offset)
            && parts[2].parse::<u64>().ok() == Some(size - 1)
            && parts[3].parse::<u64>().ok() == Some(size)
    })
}
pub(crate) fn js_number(value: &str) -> f64 {
    let value = crate::native_goal::trim(value);
    if value.is_empty() {
        return 0.0;
    }
    let radix = match value.get(..2) {
        Some("0x" | "0X") => 16,
        Some("0o" | "0O") => 8,
        Some("0b" | "0B") => 2,
        _ => return value.parse().unwrap_or(f64::NAN),
    };
    let digits = &value[2..];
    if digits.is_empty() {
        return f64::NAN;
    }
    let width = match radix {
        2 => 1,
        8 => 3,
        _ => 4,
    };
    let mut bits = Vec::with_capacity(digits.len() * width);
    for digit in digits.chars() {
        let Some(digit) = digit.to_digit(radix) else {
            return f64::NAN;
        };
        for shift in (0..width).rev() {
            bits.push((digit >> shift) & 1 != 0);
        }
    }
    let Some(first) = bits.iter().position(|bit| *bit) else {
        return 0.0;
    };
    let bits = &bits[first..];
    let mut length = bits.len();
    if length > 1024 {
        return f64::INFINITY;
    }
    if length <= 53 {
        return bits
            .iter()
            .fold(0u64, |number, bit| (number << 1) | u64::from(*bit)) as f64;
    }
    // Number(radixString) rounds the exact integer once. Incremental f64
    // multiplication can overflow early for values just below the1024bit tie.
    let mut mantissa = bits[..53]
        .iter()
        .fold(0u64, |number, bit| (number << 1) | u64::from(*bit));
    let guard = bits[53];
    let sticky = bits[54..].iter().any(|bit| *bit);
    if guard && (sticky || mantissa & 1 != 0) {
        mantissa += 1;
    }
    if mantissa == 1u64 << 53 {
        mantissa >>= 1;
        length += 1;
    }
    if length > 1024 {
        return f64::INFINITY;
    }
    f64::from_bits((((length - 1 + 1023) as u64) << 52) | (mantissa & ((1u64 << 52) - 1)))
}
