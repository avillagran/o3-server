//! Differential observations from the complete pinned Effect ModelStore module.
#[path = "support/sandbox.rs"]
mod sandbox;
use o3_server::{
    speech_catalog::Model,
    speech_model_store::{
        Backend, Body, Error, Failure, FileInfo, Progress, Response, Store, Task, Transfer,
        Transport,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
const ROOT: &str = "/fixture/speech/models";
fn bytes() -> Vec<u8> {
    (0u16..64).map(|index| ((index * 7) % 256) as u8).collect()
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
#[derive(Clone)]
struct File {
    bytes: Vec<u8>,
    inode: u64,
    mtime: i64,
}
struct State {
    files: BTreeMap<PathBuf, File>,
    events: Vec<Value>,
    requests: Vec<Value>,
    next_inode: u64,
    tick: i64,
    hashes: u64,
    changed: bool,
    request: usize,
}
impl State {
    fn put(&mut self, path: PathBuf, bytes: Vec<u8>) {
        let file = File {
            bytes,
            inode: self.next_inode,
            mtime: self.tick,
        };
        self.next_inode += 1;
        self.tick += 1;
        self.files.insert(path, file);
    }
}
#[derive(Clone)]
struct Fixture {
    state: Arc<Mutex<State>>,
    config: Value,
    root: PathBuf,
}
impl Fixture {
    fn new(config: Value, model: &Model) -> Self {
        let mut state = State {
            files: BTreeMap::new(),
            events: vec![],
            requests: vec![],
            next_inode: 100,
            tick: 1000,
            hashes: 0,
            changed: false,
            request: 0,
        };
        let final_path = Path::new(ROOT).join(model.filename());
        match config["initial"].as_str() {
            Some("valid") => state.put(final_path.clone(), bytes()),
            Some("corrupt") => state.put(final_path.clone(), vec![0; 64]),
            Some("short") => state.put(final_path.clone(), bytes()[..20].to_vec()),
            _ => {}
        }
        if config["orphan"] == true {
            state.put(
                PathBuf::from(format!(
                    "{}.f660de6c-e172-4146-8ffe-d3c32eb48cd4.part",
                    final_path.display()
                )),
                bytes()[..20].to_vec(),
            );
        }
        state.put(
            PathBuf::from(format!("{}.notes.part", final_path.display())),
            b"unrelated".to_vec(),
        );
        if let Some(marker) = config["marker"].as_str() {
            if let Some(file) = state.files.get(&final_path) {
                let text = if marker == "valid" {
                    info(file).marker(model)
                } else {
                    "untrusted".into()
                };
                state.put(
                    PathBuf::from(format!("{}.verified", final_path.display())),
                    text.into_bytes(),
                );
            }
        }
        Self {
            state: Arc::new(Mutex::new(state)),
            config,
            root: ROOT.into(),
        }
    }
}
fn info(file: &File) -> FileInfo {
    FileInfo {
        regular: true,
        size: file.bytes.len() as u64,
        mtime_ms: Some(file.mtime),
        device: 7,
        inode: Some(file.inode),
    }
}
impl Backend for Fixture {
    fn directory(&self) -> &Path {
        &self.root
    }
    fn info(&self, path: &Path) -> Option<FileInfo> {
        self.state.lock().unwrap().files.get(path).map(info)
    }
    fn read_marker(&self, path: &Path) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .files
            .get(&PathBuf::from(format!("{}.verified", path.display())))
            .map(|file| String::from_utf8(file.bytes.clone()).unwrap())
    }
    fn write_marker(&self, path: &Path, marker: &str) -> Result<(), Error> {
        if self.config["markerWriteFails"] == true {
            return Err(Error::Platform("marker write failure".into()));
        }
        let marker_path = PathBuf::from(format!("{}.verified", path.display()));
        let mut state = self.state.lock().unwrap();
        state.put(marker_path.clone(), marker.as_bytes().to_vec());
        state
            .events
            .push(json!({"kind":"marker","path":marker_path,"text":marker}));
        Ok(())
    }
    fn digest<'a>(&'a self, path: &'a Path) -> Task<'a, Option<String>> {
        Box::pin(async move {
            let mut state = self.state.lock().unwrap();
            state.hashes += 1;
            state.events.push(json!({"kind":"hash","path":path}));
            let bytes = state.files.get(path)?.bytes.clone();
            if self.config["changeDuringHash"] == true && !state.changed {
                state.files.get_mut(path)?.mtime += 100;
                state.changed = true;
            }
            Some(sha(&bytes))
        })
    }
    fn stage<'a>(&'a self, model: &'a Model) -> Task<'a, Result<Box<dyn Transfer>, Error>> {
        Box::pin(async move {
            let path = self.root.join(format!(
                "{}.00000000-0000-4000-8000-000000000001.part",
                model.filename()
            ));
            Ok(Box::new(FakeTransfer {
                fixture: self.clone(),
                path,
                published: false,
            }) as Box<dyn Transfer>)
        })
    }
    fn remove<'a>(&'a self, _: &'a Model) -> Task<'a, Result<(), Error>> {
        Box::pin(async { panic!("this differential sequence does not request explicit removal") })
    }
}
struct FakeTransfer {
    fixture: Fixture,
    path: PathBuf,
    published: bool,
}
impl Drop for FakeTransfer {
    fn drop(&mut self) {
        if !self.published {
            let mut state = self.fixture.state.lock().unwrap();
            state
                .events
                .push(json!({"kind":"remove","path":self.path,"options":{"force":true}}));
            state.files.remove(&self.path);
        }
    }
}
impl Transfer for FakeTransfer {
    fn path(&self) -> &Path {
        &self.path
    }
    fn offset(&self) -> u64 {
        self.fixture
            .state
            .lock()
            .unwrap()
            .files
            .get(&self.path)
            .map_or(0, |file| file.bytes.len() as u64)
    }
    fn restart(&mut self) -> Task<'_, Result<(), Error>> {
        Box::pin(async move {
            self.fixture
                .state
                .lock()
                .unwrap()
                .put(self.path.clone(), vec![]);
            Ok(())
        })
    }
    fn append<'a>(&'a mut self, chunk: &'a [u8]) -> Task<'a, Result<(), Error>> {
        Box::pin(async move {
            let mut state = self.fixture.state.lock().unwrap();
            let tick = state.tick;
            state.tick += 1;
            let file = state.files.get_mut(&self.path).unwrap();
            file.bytes.extend_from_slice(chunk);
            file.mtime = tick;
            Ok(())
        })
    }
    fn publish<'a>(&'a mut self, path: &'a Path) -> Task<'a, Result<(), Error>> {
        Box::pin(async move {
            let mut state = self.fixture.state.lock().unwrap();
            let file = state.files.remove(&self.path).unwrap();
            state.files.insert(path.to_owned(), file);
            state
                .events
                .push(json!({"kind":"rename","from":self.path,"to":path}));
            self.published = true;
            Ok(())
        })
    }
}
struct FakeBody {
    chunks: VecDeque<Result<Vec<u8>, Error>>,
}
impl Body for FakeBody {
    fn next(&mut self) -> Task<'_, Result<Option<Vec<u8>>, Error>> {
        Box::pin(async move { self.chunks.pop_front().transpose() })
    }
}
impl Transport for Fixture {
    fn get<'a>(&'a self, url: &'a str, offset: u64) -> Task<'a, Result<Response, Error>> {
        Box::pin(async move {
            let mut state = self.state.lock().unwrap();
            state.requests.push(
                json!({"url":url,"range":if offset>0 {Some(format!("bytes={offset}-"))}else{None}}),
            );
            let responses = self.config["responses"].as_array().unwrap();
            let spec = &responses[state.request.min(responses.len() - 1)];
            state.request += 1;
            let mut chunks = VecDeque::new();
            for chunk in spec["chunks"].as_array().into_iter().flatten() {
                chunks.push_back(Ok(match chunk.as_str() {
                    Some("all") => bytes(),
                    Some("first") => bytes()[..20].to_vec(),
                    Some("rest") => bytes()[20..].to_vec(),
                    _ => chunk
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|n| n.as_u64().unwrap() as u8)
                        .collect(),
                }));
            }
            if spec["fail"] == true {
                chunks.push_back(Err(Error::Transport("synthetic transport reset".into())));
            }
            Ok(Response {
                status: spec["status"].as_u64().unwrap_or(200) as u16,
                content_range: spec["headers"]["content-range"].as_str().map(str::to_owned),
                content_length: spec["headers"]["content-length"]
                    .as_str()
                    .map(str::to_owned),
                body: Box::new(FakeBody { chunks }),
            })
        })
    }
}
fn outcome(result: Result<PathBuf, Error>) -> Value {
    match result {
        Ok(path) => json!({"ok":true,"path":path}),
        Err(error) => {
            let (reason, status) = match &error {
                Error::Download { failure, .. } => match failure {
                    Failure::Status(status) => ("status", Some(*status)),
                    Failure::Range => ("range", None),
                    Failure::Size => ("size", None),
                    Failure::Verification => ("verification", None),
                },
                _ => panic!("unexpected non-domain failure: {error}"),
            };
            json!({"ok":false,"tag":"SpeechModelDownloadError","reason":reason,"status":status,"message":error.to_string()})
        }
    }
}
#[test]
fn whole_native_model_store_traces_match_and_foreign_partials_are_retained() {
    sandbox::assert_isolated();
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/speech-model-store-4314.json")).unwrap();
    let model = Model::from_manifest(corpus["model"].clone()).unwrap();
    assert_eq!(corpus["cases"].as_array().unwrap().len(), 50);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
  for case in corpus["cases"].as_array().unwrap() {
   let fixture=Fixture::new(case["input"].clone(),&model);let store=Store::new(Arc::new(fixture.clone()),Arc::new(fixture.clone()));
   let before=json!([store.is_ready(&model),store.installed_path(&model).await,store.is_ready(&model)]);
   let progress={let fixture=fixture.clone();move|progress|fixture.state.lock().unwrap().events.push(match progress {Progress::Downloaded(bytes)=>json!({"kind":"progress","bytes":bytes}),Progress::VerificationStarted=>json!({"kind":"verify"})})};
   let (_sender,cancel)=tokio::sync::watch::channel(false);let result=outcome(store.download(&model,&progress,cancel).await);
   let after=json!([store.is_ready(&model),store.installed_path(&model).await,store.is_ready(&model)]);
   let state=fixture.state.lock().unwrap();let expected=&case["expected"];let label=case["input"]["name"].as_str().unwrap();
   assert_eq!(before,expected["before"],"before {label}");assert_eq!(result,expected["outcome"],"outcome {label}");assert_eq!(after,expected["after"],"after {label}");assert_eq!(json!(state.requests),expected["requests"],"requests {label}");assert_eq!(json!(state.hashes),expected["hashes"],"hashes {label}");
   let expected_events=expected["events"].as_array().unwrap().iter().filter(|event|!(case["input"]["orphan"]==true&&event["kind"]=="remove"&&event["path"].as_str().unwrap_or("").ends_with("f660de6c-e172-4146-8ffe-d3c32eb48cd4.part"))).cloned().collect::<Vec<_>>();
   assert_eq!(state.events,expected_events,"events {label}");
   let files=state.files.iter().filter(|(path,_)|!path.to_string_lossy().ends_with("f660de6c-e172-4146-8ffe-d3c32eb48cd4.part")).map(|(path,file)|json!({"path":path,"bytes":file.bytes.len(),"sha256":sha(&file.bytes),"text":if path.extension().is_some_and(|e|e=="verified") {Some(String::from_utf8(file.bytes.clone()).unwrap())}else{None}})).collect::<Vec<_>>();
   assert_eq!(json!(files),expected["files"],"files {label}");
   if case["input"]["orphan"]==true {assert!(state.files.keys().any(|path|path.to_string_lossy().ends_with("f660de6c-e172-4146-8ffe-d3c32eb48cd4.part")),"unknown legacy partial must remain retained");}
  }
 });
}

fn temporary() -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let path = std::env::temp_dir().join(format!(
        "speech-model-store-{}",
        o3_server::store::random_id().unwrap()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    path
}
#[test]
fn private_marker_publication_preserves_other_hardlink_bytes() {
    sandbox::assert_isolated();
    use o3_server::speech_model_store::Local;
    use std::os::unix::fs::MetadataExt;
    let root = temporary();
    let local = Arc::new(Local::open(&root).unwrap());
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/speech-model-store-4314.json")).unwrap();
    let model = Model::from_manifest(corpus["model"].clone()).unwrap();
    let path = local.directory().join(model.filename());
    std::fs::write(&path, bytes()).unwrap();
    let outside = root.join("unrelated-owned-fixture");
    let original = b"independent fixture bytes must survive";
    std::fs::write(&outside, original).unwrap();
    let marker = PathBuf::from(format!("{}.verified", path.display()));
    std::fs::hard_link(&outside, &marker).unwrap();
    let transport = Fixture::new(json!({"responses":[{"chunks":["all"]}]}), &model);
    let store = Store::new(local.clone(), Arc::new(transport));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime.block_on(store.installed_path(&model)),
        Some(path.clone())
    );
    assert_eq!(std::fs::read(&outside).unwrap(), original);
    assert_ne!(
        std::fs::metadata(&outside).unwrap().ino(),
        std::fs::metadata(&marker).unwrap().ino()
    );
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        local.info(&path).unwrap().marker(&model)
    );
    assert!(store.is_ready(&model));
}
#[test]
fn local_transfers_verify_resume_and_retain_owned_failed_staging() {
    sandbox::assert_isolated();
    use o3_server::speech_model_store::Local;
    let root = temporary();
    let local = Arc::new(Local::open(&root).unwrap());
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/speech-model-store-4314.json")).unwrap();
    let model = Model::from_manifest(corpus["model"].clone()).unwrap();
    let path = local.directory().join(model.filename());
    std::fs::write(&path, vec![0; 64]).unwrap();
    let fixture = Fixture::new(
        json!({"responses":[{"chunks":["first"],"fail":true},{"status":206,"chunks":["rest"],"headers":{"content-range":"bytes 20-63/64","content-length":"44"}}]}),
        &model,
    );
    let store = Store::new(local.clone(), Arc::new(fixture.clone()));
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(store.download(&model, &|_| {}, cancel))
            .unwrap(),
        path
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes());
    assert!(store.is_ready(&model));
    assert_eq!(
        fixture.state.lock().unwrap().requests[1]["range"],
        "bytes=20-"
    );
    assert!(std::fs::read_dir(local.directory().join(".o3-staging"))
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".o3-retained-")));
    // A later failed transfer cannot replace the existing final file. Change its
    // identity first so the native corruption cache has a new marker to inspect.
    std::fs::write(&path, b"old independent file").unwrap();
    let failed = Fixture::new(
        json!({"responses":[{"chunks":[[0,0,0,0]],"headers":{"content-length":"64"}}]}),
        &model,
    );
    let store = Store::new(local.clone(), Arc::new(failed));
    let (_sender, cancel) = tokio::sync::watch::channel(false);
    assert!(matches!(
        runtime.block_on(store.download(&model, &|_| {}, cancel)),
        Err(Error::Download {
            failure: Failure::Verification,
            ..
        })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"old independent file");
}
#[test]
fn explicit_model_removal_uses_only_recorded_exact_file_operations() {
    sandbox::assert_isolated();
    use o3_server::{
        cleanup::{Kind, Removal, RemovalPlan},
        speech_model_store::Local,
    };
    #[derive(Default)]
    struct Record(Mutex<Vec<RemovalPlan>>);
    impl Removal for Record {
        fn execute(&self, plan: &RemovalPlan) -> std::io::Result<()> {
            self.0.lock().unwrap().push(plan.clone());
            Ok(())
        }
    }
    let root = temporary();
    let record = Arc::new(Record::default());
    let local = Arc::new(Local::with_removal(&root, record.clone()).unwrap());
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/speech-model-store-4314.json")).unwrap();
    let model = Model::from_manifest(corpus["model"].clone()).unwrap();
    let path = local.directory().join(model.filename());
    let marker = PathBuf::from(format!("{}.verified", path.display()));
    std::fs::write(&path, bytes()).unwrap();
    std::fs::write(&marker, b"marker fixture").unwrap();
    let sibling = local.directory().join("unrelated.notes");
    std::fs::write(&sibling, b"retain").unwrap();
    let transport = Fixture::new(json!({"responses":[{"chunks":["all"]}]}), &model);
    let store = Store::new(local.clone(), Arc::new(transport));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(store.remove(&model)).unwrap();
    let plans = record.0.lock().unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].operations().len(), 2);
    assert!(plans[0]
        .operations()
        .iter()
        .all(|operation| operation.identity.kind == Kind::Entry
            && operation.path.parent() == Some(local.directory())));
    assert_eq!(std::fs::read(&path).unwrap(), bytes());
    assert_eq!(std::fs::read(&marker).unwrap(), b"marker fixture");
    assert_eq!(std::fs::read(&sibling).unwrap(), b"retain");
}

#[test]
fn cancellation_and_real_http_resume_keep_owned_files_and_original_model() {
    sandbox::assert_isolated();
    use o3_server::speech_model_store::{Http, Local};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Rewritten {
        http: Http,
        url: String,
    }
    impl Transport for Rewritten {
        fn get<'a>(&'a self, _: &'a str, offset: u64) -> Task<'a, Result<Response, Error>> {
            Box::pin(async move { self.http.get(&self.url, offset).await })
        }
    }
    struct Held {
        entered: Arc<tokio::sync::Notify>,
        first: bool,
    }
    impl Body for Held {
        fn next(&mut self) -> Task<'_, Result<Option<Vec<u8>>, Error>> {
            Box::pin(async move {
                if self.first {
                    self.first = false;
                    return Ok(Some(bytes()[..20].to_vec()));
                }
                self.entered.notify_one();
                std::future::pending().await
            })
        }
    }
    struct HeldTransport(Arc<tokio::sync::Notify>);
    impl Transport for HeldTransport {
        fn get<'a>(&'a self, _: &'a str, _: u64) -> Task<'a, Result<Response, Error>> {
            Box::pin(async move {
                Ok(Response {
                    status: 200,
                    content_length: Some("64".into()),
                    content_range: None,
                    body: Box::new(Held {
                        entered: self.0.clone(),
                        first: true,
                    }),
                })
            })
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
  let corpus:Value=serde_json::from_str(include_str!("fixtures/speech-model-store-4314.json")).unwrap();let model=Model::from_manifest(corpus["model"].clone()).unwrap();
  let root=temporary();let local=Arc::new(Local::open(&root).unwrap());let path=local.directory().join(model.filename());std::fs::write(&path,b"retain original model").unwrap();
  let entered=Arc::new(tokio::sync::Notify::new());let store=Arc::new(Store::new(local.clone(),Arc::new(HeldTransport(entered.clone()))));
  let (sender,cancel)=tokio::sync::watch::channel(false);let job={let store=store.clone();let model=model.clone();tokio::spawn(async move {store.download(&model,&|_|{},cancel).await})};
  tokio::time::timeout(std::time::Duration::from_secs(2),entered.notified()).await.unwrap();sender.send(true).unwrap();assert_eq!(job.await.unwrap().unwrap_err(),Error::Cancelled);
  assert_eq!(std::fs::read(&path).unwrap(),b"retain original model");
  let retained=std::fs::read_dir(local.directory().join(".o3-staging")).unwrap().map(|entry|entry.unwrap().path()).find(|path|path.file_name().unwrap().to_string_lossy().starts_with(".o3-retained-")).unwrap();
  assert_eq!(std::fs::read(retained.join("data.gguf")).unwrap(),bytes()[..20]);
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
  let server=tokio::spawn(async move {
   let mut requests=Vec::new();
   for attempt in 0..2 {
    let (mut socket,_)=listener.accept().await.unwrap();let mut header=Vec::new();
    while !header.ends_with(b"\r\n\r\n") {let mut byte=[0u8;1];socket.read_exact(&mut byte).await.unwrap();header.push(byte[0]);assert!(header.len()<8192);}
    requests.push(String::from_utf8(header).unwrap());
    if attempt==0 {socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 64\r\nConnection: close\r\n\r\n").await.unwrap();socket.write_all(&bytes()[..20]).await.unwrap();}
    else {socket.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 44\r\nContent-Range: bytes 20-63/64\r\nConnection: close\r\n\r\n").await.unwrap();socket.write_all(&bytes()[20..]).await.unwrap();}
    socket.shutdown().await.unwrap();
   }
   requests
  });
  let http=Http::new(reqwest::Client::builder().no_proxy().build().unwrap());let transport=Rewritten {http,url:format!("http://{address}/pinned-model")};
  let store=Store::new(local.clone(),Arc::new(transport));let (_sender,cancel)=tokio::sync::watch::channel(false);
  tokio::time::timeout(std::time::Duration::from_secs(3),store.download(&model,&|_|{},cancel)).await.unwrap().unwrap();
  let requests=server.await.unwrap();assert!(!requests[0].to_ascii_lowercase().contains("range:"));assert!(requests[1].to_ascii_lowercase().contains("range: bytes=20-\r\n"));assert_eq!(std::fs::read(&path).unwrap(),bytes());
 });
}
