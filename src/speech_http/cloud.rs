//! Cloud routes use the same fresh authority and typed denial sidecar as local speech.
use super::*;
use crate::cloud_speech::{self as cloud, Provider};
#[derive(Clone, Copy)]
enum Action {
    Keys,
    Models,
    SetKey(Provider),
    Transcribe(Provider),
}
impl Action {
    fn parse(path: &str) -> Option<Self> {
        let parts = path
            .strip_prefix("/api/voice/cloud/")?
            .split('/')
            .collect::<Vec<_>>();
        Some(match parts.as_slice() {
            ["keys"] => Self::Keys,
            ["openrouter", "models"] => Self::Models,
            [provider, "key"] => Self::SetKey(parse_provider(provider)?),
            [provider, "transcribe"] => Self::Transcribe(parse_provider(provider)?),
            _ => return None,
        })
    }
    fn method(self) -> Method {
        if matches!(self, Self::Keys | Self::Models) {
            Method::GET
        } else {
            Method::POST
        }
    }
    fn scope(self) -> &'static str {
        if matches!(self, Self::SetKey(_)) {
            "settings:write"
        } else {
            "orchestration:operate"
        }
    }
}
fn parse_provider(value: &str) -> Option<Provider> {
    let mut decoded = Vec::new();
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            decoded.push(digit(input.next()?)? * 16 + digit(input.next()?)?);
        } else {
            decoded.push(byte);
        }
    }
    Provider::parse(std::str::from_utf8(&decoded).ok()?)
}

pub(super) fn matches(path: &str) -> bool {
    path.starts_with("/api/voice/cloud/")
}
fn key(value: &Value) -> Option<Option<Value>> {
    let value = value.as_object()?.get("key")?;
    if value.is_null() {
        return Some(None);
    }
    if let Some(value) = value.as_str() {
        let value = trim(value);
        return cloud::valid_api_key(value).then(|| Some(json!(value)));
    }
    crate::secret_resolver::SecretReference::decode(value)
        .map(|reference| Some(reference.to_value()))
}
fn trim(value: &str) -> &str {
    value.trim_matches(|c|matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}'))
}
fn model(query: &str) -> Option<Option<String>> {
    let values = url::form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key == "model")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Some(None),
        [value] if cloud::valid_model_id(value) => Some(Some(value.clone())),
        _ => None,
    }
}
fn failed(authority: &Authority, cause: cloud::Error) -> Reply {
    if let Some(denial) = authority.denied() {
        return denied(denial);
    }
    match cause {
        cloud::Error::Cloud { .. } => json_reply(StatusCode::UNPROCESSABLE_ENTITY, cause.encoded()),
        cloud::Error::InvalidAudio { .. } => invalid("invalid_audio"),
        _ => json_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"_tag":"EnvironmentInternalError","reason":"internal_error","traceId":trace()}),
        ),
    }
}
pub(super) async fn handle(state: Arc<State>, request: Request<Incoming>) -> Reply {
    let path = request.uri().path();
    let Some(action) = Action::parse(path) else {
        // The native route exists for an unknown provider, but its literal schema rejects it.
        let parts = path
            .strip_prefix("/api/voice/cloud/")
            .unwrap_or("")
            .split('/')
            .collect::<Vec<_>>();
        return empty(
            if matches!(parts.as_slice(), [_, "key"] | [_, "transcribe"]) {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::NOT_FOUND
            },
        );
    };
    if request.method() != action.method() || !crate::speech_state::cloud_available(&state) {
        return empty(StatusCode::NOT_FOUND);
    }
    let Some(session) = crate::http::bearer(&request)
        .and_then(|token| state.session(token))
        .filter(|session| session["jkt"].is_null())
    else {
        return denied(Denied::Invalid);
    };
    let authority = Authority::new(state.clone(), session, action.scope());
    let mut selected_model = None;
    let mut selected_key = None;
    let mut binary = None;
    if matches!(action, Action::Transcribe(_)) {
        selected_model = match model(request.uri().query().unwrap_or("")) {
            Some(value) => value,
            None => return empty(StatusCode::BAD_REQUEST),
        };
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
        binary = match collect(request, PCM_BODY_LIMIT).await {
            Ok(value) => Some(value),
            Err(()) => return empty(StatusCode::BAD_REQUEST),
        };
    } else if matches!(action, Action::SetKey(_)) {
        let content_type = media_type(&request);
        if content_type != "application/json" {
            return text(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                format!("Unsupported content-type: {content_type}"),
            );
        }
        let bytes = match collect(request, CONTROL_BODY_LIMIT).await {
            Ok(value) => value,
            Err(()) => return empty(StatusCode::BAD_REQUEST),
        };
        let decoded = String::from_utf8_lossy(&bytes);
        let value = match serde_json::from_str(decoded.strip_prefix('\u{feff}').unwrap_or(&decoded))
        {
            Ok(value) => value,
            Err(_) => return empty(StatusCode::BAD_REQUEST),
        };
        selected_key = match key(&value) {
            Some(value) => Some(value),
            None => return empty(StatusCode::BAD_REQUEST),
        };
    }
    if authority.check().is_err() {
        return denied(authority.denied().unwrap_or(Denied::Invalid));
    }
    let Some(service) = state.speech.cloud() else {
        return empty(StatusCode::NOT_FOUND);
    };
    let call = service.bind(Arc::new(authority.clone()));
    let result = match action {
        Action::Keys => call.configured().await,
        Action::Models => call.models().await.map(|models| json!({"models":models})),
        Action::SetKey(provider) => {
            return match call.set_key(provider, selected_key.unwrap()).await {
                Ok(()) => empty(StatusCode::NO_CONTENT),
                Err(cause) => failed(&authority, cause),
            }
        }
        Action::Transcribe(provider) => {
            let binary = binary.unwrap();
            let decoded = match crate::speech_request::BinaryRequest::decode(&binary) {
                Ok(value) => value,
                Err(_) => return invalid("invalid_audio"),
            };
            call.transcribe(
                provider,
                decoded.pcm.to_vec(),
                decoded.options,
                selected_model,
            )
            .await
            .map(|text| json!({"text":text}))
        }
    };
    match result {
        Ok(value) => json_reply(StatusCode::OK, value),
        Err(cause) => failed(&authority, cause),
    }
}
