//! Explicit virtual store effects for HTTP authorization; no real file publication/removal.
use o3_server::{
    speech_catalog::{self, Model},
    speech_model_store as store,
};
use std::{
    collections::HashSet,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
pub struct Memory {
    pub ready: Mutex<HashSet<String>>,
    pub requests: AtomicUsize,
    pub urls: Mutex<Vec<String>>,
}
impl Memory {
    pub fn new() -> Self {
        Self {
            ready: Mutex::new(HashSet::from(["handy-computer/moonshine-tiny-gguf".into()])),
            requests: AtomicUsize::new(0),
            urls: Mutex::new(vec![]),
        }
    }
}
impl store::Backend for Memory {
    fn directory(&self) -> &Path {
        Path::new("/fixture/virtual-models")
    }
    fn info(&self, path: &Path) -> Option<store::FileInfo> {
        speech_catalog::all()
            .iter()
            .find(|model| {
                path.file_name() == Some(std::ffi::OsStr::new(model.filename()))
                    && self.ready.lock().unwrap().contains(model.id())
            })
            .map(|model| store::FileInfo {
                regular: true,
                size: model.size(),
                mtime_ms: Some(1000),
                device: 1,
                inode: Some(2),
            })
    }
    fn read_marker(&self, _: &Path) -> Option<String> {
        None
    }
    fn write_marker(&self, _: &Path, _: &str) -> Result<(), store::Error> {
        Ok(())
    }
    fn digest<'a>(&'a self, path: &'a Path) -> store::Task<'a, Option<String>> {
        Box::pin(async move {
            speech_catalog::all()
                .iter()
                .find(|model| {
                    path.file_name() == Some(std::ffi::OsStr::new(model.filename()))
                        && self.ready.lock().unwrap().contains(model.id())
                })
                .map(|model| model.sha256().into())
        })
    }
    fn stage<'a>(
        &'a self,
        _: &'a Model,
    ) -> store::Task<'a, Result<Box<dyn store::Transfer>, store::Error>> {
        Box::pin(async { Ok(Box::new(Unused) as Box<dyn store::Transfer>) })
    }
    fn remove<'a>(&'a self, model: &'a Model) -> store::Task<'a, Result<(), store::Error>> {
        Box::pin(async move {
            self.ready.lock().unwrap().remove(model.id());
            Ok(())
        })
    }
}
struct Unused;
impl store::Transfer for Unused {
    fn path(&self) -> &Path {
        Path::new("/fixture/unused-virtual-stage")
    }
    fn offset(&self) -> u64 {
        0
    }
    fn restart(&mut self) -> store::Task<'_, Result<(), store::Error>> {
        Box::pin(async { Ok(()) })
    }
    fn append<'a>(&'a mut self, _: &'a [u8]) -> store::Task<'a, Result<(), store::Error>> {
        Box::pin(async { panic!("HTTP500 body must not be published") })
    }
    fn publish<'a>(&'a mut self, _: &'a Path) -> store::Task<'a, Result<(), store::Error>> {
        Box::pin(async { panic!("HTTP500 body must not be published") })
    }
}
impl store::Body for Unused {
    fn next(&mut self) -> store::Task<'_, Result<Option<Vec<u8>>, store::Error>> {
        Box::pin(async { panic!("HTTP500 body must not be consumed") })
    }
}
impl store::Transport for Memory {
    fn get<'a>(
        &'a self,
        url: &'a str,
        _: u64,
    ) -> store::Task<'a, Result<store::Response, store::Error>> {
        Box::pin(async move {
            self.requests.fetch_add(1, Ordering::SeqCst);
            self.urls.lock().unwrap().push(url.into());
            Ok(store::Response {
                status: 500,
                content_range: None,
                content_length: None,
                body: Box::new(Unused),
            })
        })
    }
}
