//! Local speech HTTP controllers. Cloud credentials and providers are separate.
use crate::{
    body,
    body::Body,
    speech_service as speech,
    speech_state::{Authority, Denied},
    store::State,
};
use http_body_util::{BodyExt, Limited};
use hyper::{body::Incoming, header, Method, Request, Response, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;

mod socket;
const PCM_BODY_LIMIT: usize =
    crate::speech_kernel::MAX_SPEECH_BYTES + crate::speech_request::MAX_OPTIONS_BYTES + 4;
// Control fields are much smaller; ignored JSON properties cannot force unlimited buffering.
const CONTROL_BODY_LIMIT: usize = 128_000;
type Reply = Response<Body>;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Status,
    Models,
    Prepare,
    Download,
    Select,
    Cancel,
    Remove,
    Acceleration,
    Unload,
    Transcribe,
}
impl Action {
    fn parse(path: &str) -> Option<Self> {
        Some(match path {
            "/api/voice/status" => Self::Status,
            "/api/voice/models" => Self::Models,
            "/api/voice/models/prepare" => Self::Prepare,
            "/api/voice/models/download" => Self::Download,
            "/api/voice/models/select" => Self::Select,
            "/api/voice/models/cancel" => Self::Cancel,
            "/api/voice/models/remove" => Self::Remove,
            "/api/voice/acceleration" => Self::Acceleration,
            "/api/voice/model-unload-timeout" => Self::Unload,
            "/api/voice/transcribe" => Self::Transcribe,
            _ => return None,
        })
    }
    fn method(self) -> Method {
        if matches!(self, Self::Status | Self::Models) {
            Method::GET
        } else {
            Method::POST
        }
    }
    fn scope(self) -> &'static str {
        if matches!(
            self,
            Self::Download
                | Self::Select
                | Self::Cancel
                | Self::Remove
                | Self::Acceleration
                | Self::Unload
        ) {
            "settings:write"
        } else {
            "orchestration:operate"
        }
    }
}
pub(crate) fn matches(path: &str) -> bool {
    path == "/ws/voice" || Action::parse(path).is_some()
}
fn empty(status: StatusCode) -> Reply {
    Response::builder()
        .status(status)
        .body(body::empty())
        .unwrap()
}
fn text(status: StatusCode, value: String) -> Reply {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(body::full(value))
        .unwrap()
}
fn json_reply(status: StatusCode, value: Value) -> Reply {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body::full(value.to_string()))
        .unwrap()
}
fn trace() -> String {
    crate::store::random_id().unwrap_or_default()
}
fn invalid(reason: &str) -> Reply {
    json_reply(
        StatusCode::BAD_REQUEST,
        json!({"_tag":"EnvironmentRequestInvalidError","code":"invalid_request","reason":reason,"traceId":trace()}),
    )
}
fn denied(denial: Denied) -> Reply {
    match denial {
        Denied::Invalid => json_reply(
            StatusCode::UNAUTHORIZED,
            json!({"_tag":"EnvironmentAuthInvalidError","code":"auth_invalid","reason":"invalid_credential","traceId":trace()}),
        ),
        Denied::Scope(scope) => json_reply(
            StatusCode::FORBIDDEN,
            json!({"_tag":"EnvironmentScopeRequiredError","code":"insufficient_scope","requiredScope":if scope=="settings:write"{"orchestration:operate"}else{scope},"requiredPermission":scope,"traceId":trace()}),
        ),
    }
}
fn failed(authority: &Authority, cause: speech::Error) -> Reply {
    if let Some(denial) = authority.denied() {
        return denied(denial);
    }
    match cause {
        speech::Error::InvalidAudio { .. } => invalid("invalid_audio"),
        speech::Error::Unsupported { .. } => invalid("speech_unavailable"),
        speech::Error::Busy { .. } => invalid("speech_busy"),
        speech::Error::ModelNotFound { .. } => invalid("invalid_command"),
        _ => json_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"_tag":"EnvironmentInternalError","code":"internal_error","reason":"internal_error","traceId":trace()}),
        ),
    }
}
fn media_type(request: &Request<Incoming>) -> String {
    let value = request
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|value| {
            value
                .as_bytes()
                .iter()
                .map(|byte| char::from(*byte))
                .collect::<String>()
        })
        .unwrap_or_else(|| "application/json".into());
    let lower = value.to_lowercase();
    crate::native_goal::trim(lower.split(';').next().unwrap()).into()
}
fn json_control(action: Action, value: Value) -> Option<String> {
    let object = value.as_object()?;
    match action {
        Action::Download | Action::Select | Action::Cancel | Action::Remove => {
            let id = object.get("modelId")?.as_str()?;
            (1..=160)
                .contains(&id.encode_utf16().count())
                .then(|| id.to_owned())
        }
        Action::Acceleration => crate::managed_settings_codec::decode_field(
            "speechAcceleration",
            Some(object.get("acceleration")?.clone()),
        )
        .ok()?
        .as_str()
        .map(str::to_owned),
        Action::Unload => crate::managed_settings_codec::decode_field(
            "speechModelUnloadTimeout",
            Some(object.get("timeout")?.clone()),
        )
        .ok()?
        .as_str()
        .map(str::to_owned),
        _ => None,
    }
}
fn query_benchmarks(query: &str) -> Option<bool> {
    let values = url::form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key == "benchmarks")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    if values.len() > 1 || values.first().is_some_and(|value| value != "1") {
        None
    } else {
        Some(!values.is_empty())
    }
}
async fn collect(request: Request<Incoming>, limit: usize) -> Result<hyper::body::Bytes, ()> {
    tokio::time::timeout(
        std::time::Duration::from_secs(300),
        Limited::new(request.into_body(), limit).collect(),
    )
    .await
    .map_err(|_| ())?
    .map(|body| body.to_bytes())
    .map_err(|_| ())
}
pub(crate) async fn handle(state: Arc<State>, request: Request<Incoming>) -> Reply {
    if request.uri().path() == "/ws/voice" {
        return socket::upgrade(state, request).await;
    }
    let Some(action) = Action::parse(request.uri().path()) else {
        return empty(StatusCode::NOT_FOUND);
    };
    if request.method() != action.method() || !crate::speech_state::available(&state) {
        return empty(StatusCode::NOT_FOUND);
    }
    let Some(session) = crate::http::bearer(&request)
        .and_then(|token| state.session(token))
        .filter(|session| session["jkt"].is_null())
    else {
        return denied(Denied::Invalid);
    };
    let authority = Authority::new(state, session, action.scope());
    // Native payload/query decoding occurs before the handler's scope guard.
    let mut argument = None;
    let mut pcm = None;
    let mut benchmarks = false;
    if action == Action::Models {
        benchmarks = match query_benchmarks(request.uri().query().unwrap_or("")) {
            Some(value) => value,
            None => return empty(StatusCode::BAD_REQUEST),
        };
    }
    if action == Action::Transcribe {
        if request
            .headers()
            .get(header::CONTENT_LENGTH)
            .is_some_and(|value| {
                crate::speech_model_store::js_number(&String::from_utf8_lossy(value.as_bytes()))
                    > PCM_BODY_LIMIT as f64
            })
        {
            return invalid("invalid_audio");
        }
        let content_type = media_type(&request);
        if content_type != "application/octet-stream" {
            return text(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                format!("Unsupported content-type: {content_type}"),
            );
        }
        pcm = match collect(request, PCM_BODY_LIMIT).await {
            Ok(bytes) => Some(bytes),
            Err(()) => return empty(StatusCode::BAD_REQUEST),
        };
    } else if matches!(
        action,
        Action::Download
            | Action::Select
            | Action::Cancel
            | Action::Remove
            | Action::Acceleration
            | Action::Unload
    ) {
        let content_type = media_type(&request);
        if content_type != "application/json" {
            return text(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                format!("Unsupported content-type: {content_type}"),
            );
        }
        let bytes = match collect(request, CONTROL_BODY_LIMIT).await {
            Ok(bytes) => bytes,
            Err(()) => return empty(StatusCode::BAD_REQUEST),
        };
        let decoded = String::from_utf8_lossy(&bytes);
        let value = match serde_json::from_str(decoded.strip_prefix('\u{feff}').unwrap_or(&decoded))
        {
            Ok(value) => value,
            Err(_) => return empty(StatusCode::BAD_REQUEST),
        };
        argument = match json_control(action, value) {
            Some(value) => Some(value),
            None => return empty(StatusCode::BAD_REQUEST),
        };
    }
    let call = match authority.bind() {
        Ok(call) => call,
        Err(cause) => return failed(&authority, cause),
    };
    let argument = argument.as_deref().unwrap_or("");
    let result = match action {
        Action::Status => call.status().await,
        Action::Models => call.models(benchmarks).await,
        Action::Prepare => {
            return match call.prepare().await {
                Ok(()) => empty(StatusCode::NO_CONTENT),
                Err(cause) => failed(&authority, cause),
            }
        }
        Action::Download => call.download(argument).await,
        Action::Select => call.select(argument).await,
        Action::Cancel => call.cancel_download(argument).await,
        Action::Remove => call.remove(argument).await,
        Action::Acceleration => call.update_acceleration(argument).await,
        Action::Unload => {
            call.update_unload(
                crate::speech_state::parse_unload(argument).expect("validated native enum"),
            )
            .await
        }
        Action::Transcribe => {
            let bytes = pcm.expect("collected PCM body");
            let request = match crate::speech_request::BinaryRequest::decode(&bytes) {
                Ok(request) => request,
                Err(_) => return invalid("invalid_audio"),
            };
            call.transcribe(request.pcm.to_vec(), request.options)
                .await
                .map(|text| json!({"text":text}))
        }
    };
    match result {
        Ok(value) => json_reply(StatusCode::OK, value),
        Err(cause) => failed(&authority, cause),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_http_admitted_control_arguments_and_queries_match_native() {
        crate::test_sandbox::assert_isolated();
        let golden: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/speech-http-admission-4314.json"
        ))
        .unwrap();
        assert_eq!(golden["cases"].as_array().unwrap().len(), 111);
        let mut admitted = 0;
        for row in golden["cases"].as_array().unwrap() {
            let Some(call) = row["calls"].as_array().unwrap().first() else {
                continue;
            };
            let input = &row["input"];
            let path = input["path"].as_str().unwrap();
            let (path, query) = path.split_once('?').unwrap_or((path, ""));
            let action = Action::parse(path).unwrap();
            let decoded = match action {
                Action::Models => json!(query_benchmarks(query).unwrap()),
                Action::Prepare => Value::Null,
                _ => json!(json_control(action, input["body"].clone()).unwrap()),
            };
            assert_eq!(decoded, call["input"], "{}", row["name"]);
            admitted += 1;
        }
        assert_eq!(admitted, 15);
    }
}
