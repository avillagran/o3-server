//! Explicit application-owned startup for the pinned local speech graph.
use crate::{
    speech_service::{self, EngineFile, WorkerEngine},
    speech_worker::{NodeRuntime, SealedFile},
    store::State,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc, time::Duration};
#[derive(Clone, Debug)]
pub struct Config {
    node: PathBuf,
    bundle: PathBuf,
}
impl Config {
    /// These flags are application-owner inputs, never settings/RPC/model fields.
    pub fn from_args(args: &[String]) -> Result<Option<Self>, String> {
        let value = |name: &str| -> Result<Option<PathBuf>, String> {
            let indices = args
                .iter()
                .enumerate()
                .filter(|(_, value)| value.as_str() == name)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if indices.len() > 1 {
                return Err(format!("Duplicate {name}"));
            }
            let Some(index) = indices.first() else {
                return Ok(None);
            };
            let path = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or_else(|| format!("{name} requires an absolute path"))?;
            Ok(Some(path))
        };
        match (value("--speech-node")?, value("--speech-bundle")?) {
            (None, None) => Ok(None),
            (Some(node), Some(bundle)) => Ok(Some(Self { node, bundle })),
            _ => Err("Both --speech-node and --speech-bundle are required".into()),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    native_revision: String,
    platform: String,
    engine_version: String,
    node_version: String,
    node_sha256: String,
    entry: PathBuf,
    package_pins_sha256: String,
    files: Vec<FilePin>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePin {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}
/// No acquisition or inference occurs here. All installed package bytes must
/// match the compiled archive-derived allowlist before the resource is installed.
pub async fn install(state: Arc<State>, config: Config) -> Result<(), String> {
    if !cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        return Err("The admitted speech graph supports linux-x64-gnu only".into());
    }
    let directory = state.dir.clone();
    let (local, engine) = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let manifest: Manifest =
            serde_json::from_str(include_str!("../runtime/speech/engine-files-4314.json"))
                .map_err(|cause| cause.to_string())?;
        if manifest.native_revision != "83a3674ffd84ffb33de768210c806cb1ac53fd80"
            || manifest.platform != "linux-x64-gnu"
            || manifest.engine_version != "0.4.1"
            || manifest.node_version != "26.13.0"
            || manifest.files.len() != 140
            || manifest.entry != std::path::Path::new("node_modules/transcribe-cpp/dist/index.js")
            || manifest.package_pins_sha256
                != format!(
                    "{:x}",
                    Sha256::digest(include_bytes!(
                        "../runtime/speech/engine-packages-4314.json"
                    ))
                )
        {
            return Err("Invalid compiled speech runtime pins".into());
        }
        let node_size = std::fs::metadata(&config.node)
            .map_err(|cause| cause.to_string())?
            .len();
        if node_size > 512 * 1024 * 1024 {
            return Err("Invalid admitted Node executable size".into());
        }
        let admitted = SealedFile::admit(&config.node, &manifest.node_sha256, node_size)
            .map_err(|cause| cause.to_string())?;
        let files = manifest
            .files
            .into_iter()
            .map(|pin| {
                let source = config.bundle.join("graph").join(&pin.path);
                // Full admission now, followed by independent re-admission per load.
                SealedFile::admit(&source, &pin.sha256, pin.bytes)
                    .map_err(|cause| cause.to_string())?;
                Ok(EngineFile {
                    relative: pin.path,
                    source,
                    sha256: pin.sha256,
                    bytes: pin.bytes,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let local = crate::speech_model_store::Local::open(&directory)
            .map_err(|cause| cause.to_string())?;
        let node = NodeRuntime::capture_or_reuse(
            admitted,
            &directory
                .join("speech")
                .join(format!("node-26.13.0-{}", manifest.node_sha256)),
        )
        .map_err(|cause| cause.to_string())?;
        let engine = WorkerEngine {
            node,
            entry: manifest.entry,
            files,
            directory,
            startup_deadline: Duration::from_secs(10),
            operation_deadline: Duration::from_secs(120),
            close_deadline: Duration::from_secs(5),
        };
        Ok((local, engine))
    })
    .await
    .map_err(|cause| cause.to_string())??;
    let store = Arc::new(crate::speech_model_store::Store::new(
        Arc::new(local),
        Arc::new(crate::speech_model_store::Http::new(reqwest::Client::new())),
    ));
    let service = speech_service::Service::new(
        store,
        Arc::new(engine),
        Arc::new(speech_service::WallClock),
        None,
    );
    crate::speech_state::install(&state, service)
}
