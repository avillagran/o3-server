//! Native store failure identity stays private behind the service operation error.
#[path = "support/sandbox.rs"]
mod sandbox;
use o3_server::{speech_catalog::Model, speech_model_store as store, speech_service as service};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};
struct Virtual;
impl store::Backend for Virtual {
    fn directory(&self) -> &Path {
        Path::new("/fixture/virtual-only")
    }
    fn info(&self, _: &Path) -> Option<store::FileInfo> {
        None
    }
    fn read_marker(&self, _: &Path) -> Option<String> {
        None
    }
    fn write_marker(&self, _: &Path, _: &str) -> Result<(), store::Error> {
        unreachable!("failed response cannot publish a marker")
    }
    fn digest<'a>(&'a self, _: &'a Path) -> store::Task<'a, Option<String>> {
        Box::pin(async { None })
    }
    fn stage<'a>(
        &'a self,
        _: &'a Model,
    ) -> store::Task<'a, Result<Box<dyn store::Transfer>, store::Error>> {
        Box::pin(async { Ok(Box::new(Virtual) as Box<dyn store::Transfer>) })
    }
    fn remove<'a>(&'a self, _: &'a Model) -> store::Task<'a, Result<(), store::Error>> {
        Box::pin(async { unreachable!("download does not remove models") })
    }
}
impl store::Transfer for Virtual {
    fn path(&self) -> &Path {
        Path::new("/fixture/virtual-only/owned-staging")
    }
    fn offset(&self) -> u64 {
        0
    }
    fn restart(&mut self) -> store::Task<'_, Result<(), store::Error>> {
        Box::pin(async { Ok(()) })
    }
    fn append<'a>(&'a mut self, _: &'a [u8]) -> store::Task<'a, Result<(), store::Error>> {
        Box::pin(async { unreachable!("status500 body cannot be appended") })
    }
    fn publish<'a>(&'a mut self, _: &'a Path) -> store::Task<'a, Result<(), store::Error>> {
        Box::pin(async { unreachable!("status500 cannot publish") })
    }
}
impl store::Transport for Virtual {
    fn get<'a>(
        &'a self,
        _: &'a str,
        _: u64,
    ) -> store::Task<'a, Result<store::Response, store::Error>> {
        Box::pin(async {
            Ok(store::Response {
                status: 500,
                content_range: None,
                content_length: None,
                body: Box::new(Virtual),
            })
        })
    }
}
impl store::Body for Virtual {
    fn next(&mut self) -> store::Task<'_, Result<Option<Vec<u8>>, store::Error>> {
        Box::pin(async { unreachable!("status500 body cannot be read") })
    }
}
impl service::Engine for Virtual {
    fn load<'a>(
        &'a self,
        _: &'a Model,
        _: &'a Path,
        _: &'a str,
    ) -> service::Task<'a, Result<service::Loaded, service::Error>> {
        Box::pin(async { unreachable!("download cannot launch an engine") })
    }
    fn devices(&self) -> service::Task<'_, Result<Value, service::Error>> {
        Box::pin(async { unreachable!("failed download cannot inspect devices") })
    }
}
impl service::Settings for Virtual {
    fn read<'a>(
        &'a self,
        _: &'static str,
    ) -> service::Task<'a, Result<service::Snapshot, service::Error>> {
        Box::pin(async {
            Ok(service::Snapshot {
                model_id: o3_server::speech_catalog::DEFAULT_MODEL_ID.into(),
                acceleration: "cpu".into(),
                unload: service::Unload::Min15,
            })
        })
    }
    fn patch<'a>(
        &'a self,
        _: &'static str,
        _: Value,
    ) -> service::Task<'a, Result<(), service::Error>> {
        Box::pin(async { unreachable!("download cannot patch settings") })
    }
}
#[tokio::test]
async fn private_download_error_maps_to_native_operation_and_http_failure() {
    sandbox::assert_isolated();
    let native: Value =
        serde_json::from_str(include_str!("fixtures/speech-http-4314.json")).unwrap();
    assert_eq!(
        native["actualPrivateStoreError"],
        "SpeechModelDownloadError"
    );
    assert_eq!(native["serviceError"]["tag"], "SpeechOperationError");
    let virtuals = Arc::new(Virtual);
    let service = service::Service::new(
        Arc::new(store::Store::new(virtuals.clone(), virtuals.clone())),
        virtuals.clone(),
        Arc::new(service::WallClock),
        None,
    );
    let error = service
        .bind(virtuals)
        .download(o3_server::speech_catalog::DEFAULT_MODEL_ID)
        .await
        .unwrap_err();
    match error {
        service::Error::Operation { operation, cause } => {
            assert_eq!(json!(operation), native["serviceError"]["operation"]);
            assert_eq!(cause, "Speech model download failed: status 500.");
        }
        other => panic!("private store error leaked as service variant {other:?}"),
    }
    assert_eq!(native["cases"][0]["status"], 403);
    assert_eq!(
        native["cases"][0]["body"]["requiredScope"],
        "orchestration:operate"
    );
    assert_eq!(
        native["cases"][0]["body"]["requiredPermission"],
        "settings:write"
    );
    assert_eq!(native["cases"][1]["status"], 500);
    assert_eq!(
        native["cases"][1]["body"],
        json!({"_tag":"EnvironmentInternalError","code":"internal_error","reason":"internal_error","traceId":"<native-trace>"})
    );
    service.shutdown().await.unwrap();
}
