//! Opt-in private acceptance executable; never runs during ordinary Cargo tests.
use o3_server::{
    speech_kernel::Pcm,
    speech_worker::{Cleanup, EnginePackage, NodeRuntime, SealedFile, Worker, WorkerSpec},
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
fn sealed(path: &Path, sha: &str, bytes: u64) -> SealedFile {
    SealedFile::admit(path, sha, bytes).unwrap()
}
fn runroot() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "real-speech-{}",
        o3_server::store::random_id().unwrap()
    ));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    root
}
fn main() {
    assert!(
        Path::new("/run/.containerenv").exists(),
        "Real engine acceptance requires private rootless Podman"
    );
    assert_eq!(
        std::env::var("O3_ADMITTED_NODE_VERSION").unwrap(),
        "26.13.0"
    );
    let input = PathBuf::from(std::env::var("O3_SPEECH_INPUT").unwrap());
    let receipt: Value =
        serde_json::from_slice(&fs::read(input.join("engine-receipt.json")).unwrap()).unwrap();
    assert_eq!(
        receipt["nativeRevision"],
        "83a3674ffd84ffb33de768210c806cb1ac53fd80"
    );
    assert_eq!(receipt["platform"], "linux-x64-gnu");
    let mut files = Vec::new();
    for package in receipt["packages"].as_array().unwrap() {
        assert_eq!(package["installScriptsExecuted"], false);
        for file in package["files"].as_array().unwrap() {
            let relative = PathBuf::from(file["path"].as_str().unwrap());
            files.push((
                relative.clone(),
                sealed(
                    &input.join("graph").join(relative),
                    file["sha256"].as_str().unwrap(),
                    file["bytes"].as_u64().unwrap(),
                ),
            ));
        }
    }
    let count = files.len();
    let graph =
        EnginePackage::admit("node_modules/transcribe-cpp/dist/index.js".into(), files).unwrap();
    let model = sealed(
        &input.join("model.gguf"),
        "2fd348d7b38f97d309cc3ec6848f3f57f537b80244950f07d2637e463f95a3a1",
        35_466_912,
    );
    let base = runroot();
    let node_path = PathBuf::from(std::env::var("O3_ADMITTED_NODE").unwrap());
    let node = NodeRuntime::capture(
        sealed(
            &node_path,
            &std::env::var("O3_ADMITTED_NODE_SHA256").unwrap(),
            fs::metadata(&node_path).unwrap().len(),
        ),
        &base.join("node"),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .unwrap();
    let started = Instant::now();
    let output=runtime.block_on(async move {tokio::spawn(async move {
        let worker=Worker::start(WorkerSpec {node,engine:graph,model:Some(model),vad:None,directory:base.join("runtime"),acceleration:"cpu".into(),startup_deadline:Duration::from_secs(10),operation_deadline:Duration::from_secs(60),close_deadline:Duration::from_secs(5)}).await.unwrap();
        let loaded=worker.load().await.unwrap();assert!(loaded["backend"].as_str().unwrap().eq_ignore_ascii_case("cpu"));
        let pcm=Pcm::decode(&vec![0;16_000*4]).unwrap();
        let buffered=worker.transcribe(&pcm,json!({"timestamps":"none","language":"en"}),false).await.unwrap();
        assert!(buffered["text"].is_string());
        let stream=worker.begin(json!({"timestamps":"none","language":"en"}),false).await.unwrap();
        let preview=worker.feed(stream,&pcm).await.unwrap();assert!(preview["revision"].is_number());
        let finished=worker.finish(stream).await.unwrap();assert!(finished.is_string());
        let cleanup=worker.close().await;assert!(matches!(cleanup,Cleanup::Confirmed{..}));
        json!({"engine":"transcribe-cpp0.4.1","model":"handy-computer/moonshine-tiny-gguf","graphFiles":count,"loaded":loaded,"bufferedTextBytes":buffered["text"].as_str().unwrap().len(),"streamTextBytes":finished.as_str().unwrap().len(),"streamRevision":preview["revision"],"cleanup":format!("{cleanup:?}"),"pcm":"one second of synthetic silence; no microphone or recording","capabilitiesEnabled":false})
    }).await.unwrap()});
    println!(
        "{}",
        json!({"result":output,"elapsedMilliseconds":started.elapsed().as_millis()})
    );
}
