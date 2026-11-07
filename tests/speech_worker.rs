//! Actual Rust ownership around the whole native factory and a synthetic engine.
#[path = "support/sandbox.rs"]
mod sandbox;
use o3_server::{
    speech_kernel::Pcm,
    speech_worker::{Cleanup, EnginePackage, Error, NodeRuntime, SealedFile, Worker, WorkerSpec},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};
const CLOSE_DEADLINE: Duration = Duration::from_secs(2);

fn watch_native_exit(pid: u32) -> tokio::io::unix::AsyncFd<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // Capture the owned child before the crash/cancel. A pidfd cannot be
    // redirected by PID reuse, and its readiness proves exit without requiring
    // the container init to reap an orphan on a particular scheduler turn.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, i32::try_from(pid).unwrap(), 0) };
    assert!(descriptor >= 0, "owned native child needs an exit receipt");
    let owned = unsafe { OwnedFd::from_raw_fd(i32::try_from(descriptor).unwrap()) };
    let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    tokio::io::unix::AsyncFd::new(owned).unwrap()
}

async fn assert_native_exited(exit: tokio::io::unix::AsyncFd<std::os::fd::OwnedFd>) {
    let _exit_ready = tokio::time::timeout(CLOSE_DEADLINE, exit.readable())
        .await
        .expect("group fence must terminate the owned native child")
        .expect("owned native exit observation failed");
}
fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "speech-owner-{}",
        o3_server::store::random_id().unwrap()
    ));
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
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
        let admitted = SealedFile::admit(
            &path,
            &std::env::var("O3_ADMITTED_NODE_SHA256").unwrap(),
            fs::metadata(&path).unwrap().len(),
        )
        .unwrap();
        NodeRuntime::capture(admitted, &root().join("node")).unwrap()
    })
    .clone()
}
struct Fixture {
    base: PathBuf,
    worker: Worker,
}
impl Fixture {
    async fn start(mode: &str) -> Self {
        Self::start_mode(mode, true).await
    }
    async fn start_mode(mode: &str, with_model: bool) -> Self {
        let base = root();
        let engine = base.join("engine.mjs");
        let model = base.join("model");
        fs::write(&engine, b"export * from './lib/synthetic.mjs';").unwrap();
        fs::create_dir(base.join("lib")).unwrap();
        let dependency = base.join("lib/synthetic.mjs");
        fs::write(&dependency, include_bytes!("speech/synthetic-engine.mjs")).unwrap();
        fs::write(&model, mode).unwrap();
        let package = EnginePackage::admit(
            "engine.mjs".into(),
            vec![
                ("engine.mjs".into(), seal(&engine)),
                ("lib/synthetic.mjs".into(), seal(&dependency)),
            ],
        )
        .unwrap();
        let worker = Worker::start(WorkerSpec {
            node: node(),
            engine: package,
            model: with_model.then(|| seal(&model)),
            vad: None,
            directory: base.join("run"),
            acceleration: "cpu".into(),
            startup_deadline: Duration::from_secs(10),
            operation_deadline: Duration::from_secs(5),
            close_deadline: CLOSE_DEADLINE,
        })
        .await
        .unwrap();
        Self { base, worker }
    }
    fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.base.join("run/home/events.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    async fn entered(&self) -> u32 {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(text) = fs::read_to_string(self.base.join("run/home/entered")) {
                    break text.parse().unwrap();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
}
fn fresh(name: &str) -> bool {
    if std::env::var("O3_SPEECH_OWNER_CHILD").ok().as_deref() == Some(name) {
        return false;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env("O3_SPEECH_OWNER_CHILD", name)
        .status()
        .unwrap();
    assert!(status.success(), "fresh private owner case failed: {name}");
    true
}
fn assert_worker_stack() {
    unsafe {
        let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
        assert_eq!(
            libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()),
            0
        );
        let mut attributes = attributes.assume_init();
        let mut size = 0usize;
        let mut address = std::ptr::null_mut();
        assert_eq!(
            libc::pthread_attr_getstack(&attributes, &mut address, &mut size),
            0
        );
        assert_eq!(libc::pthread_attr_destroy(&mut attributes), 0);
        assert_eq!(size, 512 * 1024, "actual production stack budget");
    }
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .unwrap()
}
#[test]
fn whole_factory_max_buffer_and_independent_stream_generations() {
    sandbox::assert_isolated();
    if fresh("whole_factory_max_buffer_and_independent_stream_generations") {
        return;
    }
    runtime().block_on(async {
        // Spawn onto the production-sized worker rather than the block_on caller.
        tokio::spawn(async {
            assert_worker_stack();
            assert_worker_stack();
            assert_worker_stack();
            let fixture = Fixture::start("stream").await;
            fs::write(fixture.base.join("model"), "hang").unwrap();
            fs::write(
                fixture.base.join("lib/synthetic.mjs"),
                "throw Error('Changed source must not execute')",
            )
            .unwrap();
            fixture.worker.load().await.unwrap();
            assert!(fixture
                .events()
                .iter()
                .all(|event| event["cwd"] == fixture.base.join("run").to_str().unwrap()));
            let mut bytes = vec![0; 19_200_000];
            bytes[..4].copy_from_slice(&0.25f32.to_le_bytes());
            let size = bytes.len();
            bytes[size - 4..].copy_from_slice(&(-0.75f32).to_le_bytes());
            let pcm = Pcm::decode(&bytes).unwrap();
            let result = fixture
                .worker
                .transcribe(&pcm, json!({"timestamps":"none"}), false)
                .await
                .unwrap();
            let text: Value = serde_json::from_str(result["text"].as_str().unwrap()).unwrap();
            assert_eq!(text["bytes"], 19_200_000);
            assert_eq!(text["first"], 0.25);
            assert_eq!(text["last"], -0.75);
            let one = Pcm::decode(&1f32.to_le_bytes()).unwrap();
            let first = fixture
                .worker
                .begin(json!({"timestamps":"none","language":"en"}), false)
                .await
                .unwrap();
            fixture.worker.feed(first, &one).await.unwrap();
            fixture.worker.finish(first).await.unwrap();
            let second = fixture
                .worker
                .begin(json!({"timestamps":"none","language":"de"}), false)
                .await
                .unwrap();
            assert_eq!(
                fixture.worker.feed(first, &one).await.unwrap_err(),
                Error::Stale
            );
            fixture.worker.feed(second, &one).await.unwrap();
            fixture.worker.finish(second).await.unwrap();
            assert!(matches!(
                fixture.worker.close().await,
                Cleanup::Confirmed { .. }
            ));
            let languages = fixture
                .events()
                .into_iter()
                .filter(|x| x["kind"] == "feed")
                .map(|x| x["language"].clone())
                .collect::<Vec<_>>();
            assert_eq!(languages, vec![json!("en"), json!("de")]);
        })
        .await
        .unwrap();
    });
}
#[test]
fn cancellation_fences_hung_native_group_and_rejects_parallel_discovery() {
    sandbox::assert_isolated();
    if fresh("cancellation_fences_hung_native_group_and_rejects_parallel_discovery") {
        return;
    }
    runtime().block_on(async {
        tokio::spawn(async {
            assert_worker_stack();
            assert_worker_stack();
            assert_worker_stack();
            let fixture = Fixture::start("hang").await;
            fixture.worker.load().await.unwrap();
            let worker = fixture.worker.clone();
            let task = tokio::spawn(async move {
                worker
                    .transcribe(
                        &Pcm::decode(&0.25f32.to_le_bytes()).unwrap(),
                        json!({"timestamps":"none"}),
                        false,
                    )
                    .await
            });
            let pid = fixture.entered().await;
            let exit = watch_native_exit(pid);
            assert_eq!(fixture.worker.discover().await.unwrap_err(), Error::Busy);
            task.abort();
            let _ = task.await;
            let cleanup = fixture.worker.close().await;
            assert!(matches!(cleanup, Cleanup::Confirmed { .. }));
            assert_native_exited(exit).await;
            if matches!(cleanup, Cleanup::Confirmed { graceful: true }) {
                assert_eq!(
                    unsafe { libc::kill(pid as i32, 0) },
                    -1,
                    "graceful native disposal also reaps its child"
                );
            }
            assert_eq!(
                fixture
                    .events()
                    .iter()
                    .filter(|event| event["kind"] == "transcribe")
                    .count(),
                1
            );
        })
        .await
        .unwrap();
    });
}
#[test]
fn native_exit_and_disconnect_never_replay_audio() {
    sandbox::assert_isolated();
    if fresh("native_exit_and_disconnect_never_replay_audio") {
        return;
    }
    runtime().block_on(async {
        tokio::spawn(async {
            assert_worker_stack();
            assert_worker_stack();
            assert_worker_stack();
            for mode in ["exit", "disconnect"] {
                let fixture = Fixture::start(mode).await;
                fixture.worker.load().await.unwrap();
                let error = fixture
                    .worker
                    .transcribe(
                        &Pcm::decode(&0.25f32.to_le_bytes()).unwrap(),
                        json!({"timestamps":"none"}),
                        false,
                    )
                    .await
                    .unwrap_err();
                assert!(matches!(error, Error::Native(_)));
                assert!(matches!(
                    fixture.worker.close().await,
                    Cleanup::Confirmed { .. }
                ));
                assert_eq!(
                    fixture
                        .events()
                        .iter()
                        .filter(|event| event["kind"] == "transcribe")
                        .count(),
                    1
                );
            }
        })
        .await
        .unwrap();
    });
}

#[test]
fn sealed_graph_refuses_ambient_imports_and_preserves_sources_and_existing_paths() {
    sandbox::assert_isolated();
    if fresh("sealed_graph_refuses_ambient_imports_and_preserves_sources_and_existing_paths") {
        return;
    }
    runtime().block_on(async {tokio::spawn(async {
        assert_worker_stack();
        let base=root();let target=base.join("owned-target");fs::write(&target,"owned").unwrap();
        let shared_parent=base.join("shared-parent");fs::create_dir(&shared_parent).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&shared_parent,fs::Permissions::from_mode(0o777)).unwrap();
        assert!(NodeRuntime::capture(seal(&PathBuf::from(std::env::var("O3_ADMITTED_NODE").unwrap())),&shared_parent.join("node")).is_err());
        assert!(!shared_parent.join("node").exists());
        let nested=shared_parent.join("private-child");fs::DirBuilder::new().mode(0o700).create(&nested).unwrap();
        assert!(NodeRuntime::capture(seal(&PathBuf::from(std::env::var("O3_ADMITTED_NODE").unwrap())),&nested.join("node")).is_err());
        assert!(!nested.join("node").exists());


        std::os::unix::fs::symlink(&target,base.join("link")).unwrap();
        assert!(SealedFile::admit(&base.join("link"),&format!("{:x}",Sha256::digest(b"owned")),5).is_err());
        fs::create_dir(base.join("directory")).unwrap();
        fs::write(base.join("directory/owned"),"owned").unwrap();
        std::os::unix::fs::symlink(base.join("directory"),base.join("linked-directory")).unwrap();
        assert!(SealedFile::admit(&base.join("linked-directory/owned"),&format!("{:x}",Sha256::digest(b"owned")),5).is_err());
        assert!(EnginePackage::admit("../escape.mjs".into(),vec![("../escape.mjs".into(),seal(&target))]).is_err());
        let outside=base.join("outside.mjs");fs::write(&outside,"throw Error('Excluded file executed')").unwrap();
        let entry=base.join("entry.mjs");
        fs::write(&entry,format!("import {};",serde_json::to_string(&url::Url::from_file_path(&outside).unwrap().to_string()).unwrap())).unwrap();
        let package=EnginePackage::admit("entry.mjs".into(),vec![("entry.mjs".into(),seal(&entry))]).unwrap();
        let worker=Worker::start(WorkerSpec {node:node(),engine:package,model:Some(seal(&target)),vad:Some(seal(&target)),directory:base.join("runtime"),acceleration:"cpu".into(),startup_deadline:Duration::from_secs(10),operation_deadline:Duration::from_secs(5),close_deadline:Duration::from_secs(2)}).await.unwrap();
        let error=worker.load().await.unwrap_err();
        assert!(matches!(error,Error::Native(message) if message.contains("Unadmitted speech package import")),"guard must reject an existing excluded module");
        assert!(matches!(worker.close().await,Cleanup::Confirmed{..}));
        assert_eq!(fs::read_to_string(&target).unwrap(),"owned");
        assert_eq!(fs::read_to_string(base.join("runtime/vad")).unwrap(),"owned");
        assert!(!base.join("runtime/engine/outside.mjs").exists());
        assert!(base.join("link").symlink_metadata().unwrap().file_type().is_symlink());
    }).await.unwrap();});
}
#[test]
fn crashed_bridge_fences_remaining_native_child_without_replay() {
    sandbox::assert_isolated();
    if fresh("crashed_bridge_fences_remaining_native_child_without_replay") {
        return;
    }
    runtime().block_on(async {
        tokio::spawn(async {
            assert_worker_stack();
            let fixture = Fixture::start("hang").await;
            fixture.worker.load().await.unwrap();
            let worker = fixture.worker.clone();
            let operation = tokio::spawn(async move {
                worker
                    .transcribe(
                        &Pcm::decode(&0.25f32.to_le_bytes()).unwrap(),
                        json!({"timestamps":"none"}),
                        false,
                    )
                    .await
            });
            let native = fixture.entered().await;
            let exit = watch_native_exit(native);
            let parent = fixture
                .events()
                .iter()
                .find(|value| value["kind"] == "load")
                .unwrap()["parent"]
                .as_u64()
                .unwrap() as i32;
            assert!(parent > 1);
            assert_eq!(unsafe { libc::kill(parent, libc::SIGKILL) }, 0);
            assert!(matches!(
                operation.await.unwrap(),
                Err(Error::Closed | Error::Protocol)
            ));
            assert_eq!(
                fixture.worker.close().await,
                Cleanup::Confirmed { graceful: false }
            );
            assert_native_exited(exit).await;
            assert_eq!(
                fixture
                    .events()
                    .iter()
                    .filter(|event| event["kind"] == "transcribe")
                    .count(),
                1
            );
        })
        .await
        .unwrap();
    });
}

#[test]
fn discovery_without_model_is_one_shot_and_reaped_before_publication() {
    sandbox::assert_isolated();
    if fresh("discovery_without_model_is_one_shot_and_reaped_before_publication") {
        return;
    }
    runtime().block_on(async {
        tokio::spawn(async {
            assert_worker_stack();
            let fixture = Fixture::start_mode("unused", false).await;
            let result = fixture.worker.discover().await.unwrap();
            assert_eq!(
                result,
                json!([{"id":"[\"synthetic\",7]","name":"Synthetic GPU"}])
            );
            assert_eq!(fixture.worker.discover().await.unwrap_err(), Error::Closed);
            assert!(matches!(
                fixture.worker.close().await,
                Cleanup::Confirmed { .. }
            ));
            assert!(!fixture.base.join("run/model").exists());
        })
        .await
        .unwrap();
    });
}

#[test]
fn failed_startup_keeps_typed_terminal_cleanup_receipt() {
    sandbox::assert_isolated();
    let base = root();
    let node_path = base.join("not-node.py");
    fs::write(&node_path, b"#!/usr/bin/python3\nraise SystemExit(0)\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&node_path, fs::Permissions::from_mode(0o500)).unwrap();
    let node = NodeRuntime::capture(seal(&node_path), &base.join("executable")).unwrap();
    let engine = base.join("unused.mjs");
    fs::write(&engine, "throw Error('Must not execute')").unwrap();
    let package = EnginePackage::admit(
        "unused.mjs".into(),
        vec![("unused.mjs".into(), seal(&engine))],
    )
    .unwrap();
    runtime().block_on(async {
        let result = Worker::start(WorkerSpec {
            node,
            engine: package,
            model: None,
            vad: None,
            directory: base.join("runtime"),
            acceleration: "cpu".into(),
            startup_deadline: Duration::from_secs(2),
            operation_deadline: Duration::from_secs(2),
            close_deadline: Duration::from_secs(2),
        })
        .await;
        let error = match result {
            Ok(_) => panic!("Unadmitted runtime must not become ready"),
            Err(error) => error,
        };
        let failure = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<o3_server::speech_worker::StartupFailure>())
            .expect("Runtime failure must retain its typed cleanup receipt");
        assert!(matches!(failure.cleanup, Cleanup::Confirmed { .. }));
    });
}
