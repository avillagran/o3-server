//! Opt-in actual service/store/worker acceptance; no microphone, network or live profile.
use o3_server::{
    speech_catalog,
    speech_model_store::{Http, Local, Store},
    speech_service::{self, EngineFile, Error, Settings, Snapshot, Task, Unload, WorkerEngine},
    speech_worker::{NodeRuntime, SealedFile},
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
struct Authority(Mutex<Snapshot>);
impl Settings for Authority {
    fn read<'a>(&'a self, _: &'static str) -> Task<'a, Result<Snapshot, Error>> {
        Box::pin(async { Ok(self.0.lock().unwrap().clone()) })
    }
    fn patch<'a>(&'a self, _: &'static str, patch: Value) -> Task<'a, Result<(), Error>> {
        Box::pin(async move {
            if let Some(id) = patch["speechModelId"].as_str() {
                self.0.lock().unwrap().model_id = id.into();
            }
            Ok(())
        })
    }
}
fn main() {
    assert!(
        std::path::Path::new("/run/.containerenv").exists(),
        "Private rootless Podman only"
    );
    let input = PathBuf::from(std::env::var("O3_SPEECH_INPUT").unwrap());
    let receipt: Value =
        serde_json::from_slice(&fs::read(input.join("engine-receipt.json")).unwrap()).unwrap();
    assert_eq!(
        receipt["nativeRevision"],
        "83a3674ffd84ffb33de768210c806cb1ac53fd80"
    );
    assert_eq!(receipt["platform"], "linux-x64-gnu");
    let base = std::env::temp_dir().join(format!(
        "service-{}",
        o3_server::store::random_id().unwrap()
    ));
    fs::DirBuilder::new().mode(0o700).create(&base).unwrap();
    let node_path = PathBuf::from(std::env::var("O3_ADMITTED_NODE").unwrap());
    let node = NodeRuntime::capture(
        SealedFile::admit(
            &node_path,
            &std::env::var("O3_ADMITTED_NODE_SHA256").unwrap(),
            fs::metadata(&node_path).unwrap().len(),
        )
        .unwrap(),
        &base.join("node"),
    )
    .unwrap();
    let local = Local::open(&base).unwrap();
    let tiny = speech_catalog::get("handy-computer/moonshine-tiny-gguf").unwrap();
    fs::copy(
        input.join("model.gguf"),
        base.join("speech/models").join(tiny.filename()),
    )
    .unwrap();
    let files = receipt["packages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|package| {
            assert_eq!(package["installScriptsExecuted"], false);
            package["files"].as_array().unwrap().iter().map(|file| {
                let relative = PathBuf::from(file["path"].as_str().unwrap());
                EngineFile {
                    source: input.join("graph").join(&relative),
                    relative,
                    sha256: file["sha256"].as_str().unwrap().into(),
                    bytes: file["bytes"].as_u64().unwrap(),
                }
            })
        })
        .collect::<Vec<_>>();
    let count = files.len();
    let engine = Arc::new(WorkerEngine {
        node,
        entry: "node_modules/transcribe-cpp/dist/index.js".into(),
        files,
        directory: base.clone(),
        startup_deadline: Duration::from_secs(10),
        operation_deadline: Duration::from_secs(120),
        close_deadline: Duration::from_secs(5),
    });
    let store = Arc::new(Store::new(
        Arc::new(local),
        Arc::new(Http::new(reqwest::Client::new())),
    ));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(512 * 1024)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {tokio::spawn(async move {
  let service=speech_service::Service::new(store,engine,Arc::new(speech_service::WallClock),None);
  let settings=Arc::new(Authority(Mutex::new(Snapshot {model_id:tiny.id().into(),acceleration:"cpu".into(),unload:Unload::Min15})));
  let call=service.bind(settings.clone());let status=call.status().await.unwrap();assert_eq!(status["state"],"ready");
  call.prepare().await.unwrap();call.prepare().await.unwrap();
  let transcript=call.transcribe(vec![0;16_000*4],o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":true,"speechSilenceFiltering":false})).unwrap()).await.unwrap();
  let stream=call.start_stream(o3_server::speech_request::TranscriptionOptions::decode(json!({"speechLanguage":"en","speechTranslateToEnglish":false,"speechCustomWords":[],"speechRemoveFillerWords":true,"speechSilenceFiltering":false})).unwrap()).await.unwrap();
  let update=stream.feed(vec![0;64_000]).await.unwrap();assert!(update["revision"].as_u64().is_some());
  let stream_transcript=stream.finish().await.unwrap();assert_eq!(call.status().await.unwrap()["state"],"transcribing");
  stream.close().await.unwrap();call.prepare().await.unwrap();
  let models=call.select(speech_catalog::all()[0].id()).await.unwrap();assert!(models["models"].as_array().unwrap().iter().all(|model|model["active"]==false));
  call.select(tiny.id()).await.unwrap();call.prepare().await.unwrap();service.shutdown().await.unwrap();
  assert!(matches!(call.prepare().await,Err(Error::Busy{..})));
  println!("{}",json!({"actualWorkerAndStore":true,"engine":"transcribe-cpp0.4.1","graphFiles":count,"model":tiny.id(),"status":status,"prepareReuse":true,"bufferedTranscriptBytes":transcript.len(),"streamTranscriptBytes":stream_transcript.len(),"streamFinishedRetainsReservation":true,"streamScopeCloseConfirmed":true,"pcm":"one second of synthetic silence; no microphone","selectDisposed":true,"shutdownConfirmed":true,"closedCannotResurrect":true,"httpOrCapabilitiesEnabled":false}));
 }).await.unwrap();});
}
