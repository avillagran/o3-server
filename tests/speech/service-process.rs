//! Explicit synthetic ASR bytes behind the actual whole factory and Rust owner.
use super::{candidate, sandbox, Error, Loaded, Task};
use o3_server::{
    speech_catalog::Model,
    speech_worker::{EnginePackage, NodeRuntime, SealedFile, Worker, WorkerSpec},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
fn private() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "service-process-{}",
        o3_server::store::random_id().unwrap()
    ));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    root
}
fn seal(path: &Path) -> SealedFile {
    let bytes = fs::read(path).unwrap();
    SealedFile::admit(
        path,
        &format!("{:x}", Sha256::digest(&bytes)),
        bytes.len() as u64,
    )
    .unwrap()
}
fn node() -> Arc<NodeRuntime> {
    static NODE: OnceLock<Arc<NodeRuntime>> = OnceLock::new();
    NODE.get_or_init(|| {
        sandbox::assert_isolated();
        assert_eq!(
            std::env::var("O3_ADMITTED_NODE_VERSION").unwrap(),
            "26.13.0"
        );
        let path = PathBuf::from(std::env::var("O3_ADMITTED_NODE").unwrap());
        let bytes = fs::metadata(&path).unwrap().len();
        NodeRuntime::capture(
            SealedFile::admit(
                &path,
                &std::env::var("O3_ADMITTED_NODE_SHA256").unwrap(),
                bytes,
            )
            .unwrap(),
            &private().join("node"),
        )
        .unwrap()
    })
    .clone()
}
pub struct Engine {
    mode: &'static str,
    runs: Mutex<Vec<(PathBuf, String)>>,
}
impl Engine {
    pub fn new(mode: &'static str) -> Arc<Self> {
        Arc::new(Self {
            mode,
            runs: Mutex::new(vec![]),
        })
    }
    pub fn placements(&self) -> Vec<String> {
        self.runs
            .lock()
            .unwrap()
            .iter()
            .map(|(_, acceleration)| acceleration.clone())
            .collect()
    }
    pub fn events(&self) -> Vec<Value> {
        self.runs
            .lock()
            .unwrap()
            .iter()
            .flat_map(|(root, _)| {
                fs::read_to_string(root.join("run/home/events.jsonl"))
                    .unwrap_or_default()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    pub async fn entered(&self) -> u32 {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                for (root, _) in self.runs.lock().unwrap().iter() {
                    if let Ok(value) = fs::read_to_string(root.join("run/home/entered")) {
                        return value.parse().unwrap();
                    }
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("actual owned synthetic inference must enter")
    }
}
impl candidate::Engine for Engine {
    fn load<'a>(
        &'a self,
        _: &'a Model,
        _: &'a Path,
        acceleration: &'a str,
    ) -> Task<'a, Result<Loaded, Error>> {
        Box::pin(async move {
            let base = private();
            let entry = base.join("synthetic.mjs");
            fs::write(&entry, include_bytes!("synthetic-engine.mjs")).unwrap();
            let model = base.join("mode");
            fs::write(&model, self.mode).unwrap();
            let worker = Worker::start(WorkerSpec {
                node: node(),
                engine: EnginePackage::admit(
                    "synthetic.mjs".into(),
                    vec![("synthetic.mjs".into(), seal(&entry))],
                )
                .unwrap(),
                model: Some(seal(&model)),
                vad: None,
                directory: base.join("run"),
                acceleration: acceleration.into(),
                startup_deadline: Duration::from_secs(10),
                operation_deadline: Duration::from_secs(5),
                close_deadline: Duration::from_secs(2),
            })
            .await
            .unwrap();
            let metadata = worker.load().await.unwrap();
            self.runs.lock().unwrap().push((base, acceleration.into()));
            Ok(Loaded::from_worker(worker, metadata))
        })
    }
    fn devices(&self) -> Task<'_, Result<Value, Error>> {
        Box::pin(async { Ok(json!([])) })
    }
}
pub fn fresh(name: &str) -> bool {
    sandbox::assert_isolated();
    if std::env::var("O3_SPEECH_SERVICE_CHILD").ok().as_deref() == Some(name) {
        return false;
    }
    assert!(std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--test-threads=1", "--nocapture"])
        .env("O3_SPEECH_SERVICE_CHILD", name)
        .status()
        .unwrap()
        .success());
    true
}
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .unwrap()
}
pub fn assert_stack() {
    unsafe {
        let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
        assert_eq!(
            libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()),
            0
        );
        let mut attributes = attributes.assume_init();
        let mut bytes = 0;
        let mut address = std::ptr::null_mut();
        assert_eq!(
            libc::pthread_attr_getstack(&attributes, &mut address, &mut bytes),
            0
        );
        assert_eq!(libc::pthread_attr_destroy(&mut attributes), 0);
        assert_eq!(bytes, 512 * 1024);
    }
}
pub fn exit(pid: u32) -> tokio::io::unix::AsyncFd<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, i32::try_from(pid).unwrap(), 0) };
    assert!(fd >= 0);
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(i32::try_from(fd).unwrap()) };
    let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    // Safety: moved OwnedFd remains stable and private until AsyncFd drops.
    unsafe {
        tokio::io::unix::AsyncFd::register_with_interest(owned, tokio::io::Interest::READABLE)
    }
    .unwrap()
}
