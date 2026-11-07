//! Trusted resource installation and fresh request authority for local speech.
use crate::{speech_service as speech, store::State};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct Registry(Mutex<Option<speech::Service>>);
impl Registry {
    pub(crate) fn service(&self) -> Option<speech::Service> {
        self.0.lock().ok()?.clone()
    }
    pub(crate) fn signal_shutdown(&self) {
        if let Some(service) = self.service() {
            service.begin_shutdown();
        }
    }
}
/// Application-owned admission only. There is no request, environment variable or
/// wire decoder that can supply an engine, executable, package graph or model path.
pub fn install(state: &State, service: speech::Service) -> Result<(), String> {
    let mut slot = state
        .speech
        .0
        .lock()
        .map_err(|_| "Speech resource lock failed")?;
    if state
        .shutting_down
        .load(std::sync::atomic::Ordering::Acquire)
        || slot.is_some()
    {
        return Err("Speech resource is already installed or closing".into());
    }
    *slot = Some(service);
    Ok(())
}
pub fn available(state: &State) -> bool {
    !state
        .shutting_down
        .load(std::sync::atomic::Ordering::Acquire)
        && state.speech.service().is_some()
}
pub async fn shutdown(state: &State) -> Result<(), speech::Error> {
    match state.speech.service() {
        Some(service) => service.shutdown().await,
        None => Ok(()),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Denied {
    Invalid,
    Scope(&'static str),
}
#[derive(Clone)]
pub(crate) struct Authority {
    state: Arc<State>,
    session: Value,
    required: &'static str,
    denied: Arc<Mutex<Option<Denied>>>,
}
impl Authority {
    pub(crate) fn new(state: Arc<State>, session: Value, required: &'static str) -> Self {
        Self {
            state,
            session,
            required,
            denied: Arc::new(Mutex::new(None)),
        }
    }
    fn fail(&self, denial: Denied) -> speech::Error {
        if let Ok(mut denied) = self.denied.lock() {
            *denied = Some(denial);
        }
        speech::Error::Operation {
            operation: "authority",
            cause: "Current speech authority refused".into(),
        }
    }
    pub(crate) fn check(&self) -> Result<(), speech::Error> {
        self.check_required(self.required)
    }
    fn check_required(&self, required: &'static str) -> Result<(), speech::Error> {
        let session = self
            .state
            .current_session(&self.session)
            .ok_or_else(|| self.fail(Denied::Invalid))?;
        if !session["scopes"]
            .as_array()
            .is_some_and(|scopes| scopes.iter().any(|scope| scope == required))
        {
            return Err(self.fail(Denied::Scope(required)));
        }
        Ok(())
    }
    pub(crate) fn denied(&self) -> Option<Denied> {
        self.denied.lock().ok().and_then(|value| *value)
    }
    pub(crate) fn bind(&self) -> Result<speech::Call, speech::Error> {
        self.check()?;
        self.state
            .speech
            .service()
            .map(|service| service.bind(Arc::new(self.clone())))
            .ok_or_else(|| speech::Error::Operation {
                operation: "resource",
                cause: "Speech resource is not installed".into(),
            })
    }
}
impl speech::Settings for Authority {
    fn read<'a>(
        &'a self,
        operation: &'static str,
    ) -> speech::Task<'a, Result<speech::Snapshot, speech::Error>> {
        Box::pin(async move {
            self.check()?;
            let settings = crate::settings::read(&self.state)
                .map_err(|cause| speech::Error::Operation { operation, cause })?;
            let field = |name| {
                crate::managed_settings_codec::decode_field(name, settings.get(name).cloned())
                    .map_err(|()| speech::Error::Operation {
                        operation,
                        cause: "Invalid effective speech settings".into(),
                    })
            };
            let model = field("speechModelId")?;
            let acceleration = field("speechAcceleration")?;
            let unload = field("speechModelUnloadTimeout")?;
            self.check()?;
            Ok(speech::Snapshot {
                model_id: model.as_str().unwrap().into(),
                acceleration: acceleration.as_str().unwrap().into(),
                unload: parse_unload(unload.as_str().unwrap()).ok_or_else(|| {
                    speech::Error::Operation {
                        operation,
                        cause: "Invalid effective unload timeout".into(),
                    }
                })?,
            })
        })
    }
    fn patch<'a>(
        &'a self,
        operation: &'static str,
        patch: Value,
    ) -> speech::Task<'a, Result<(), speech::Error>> {
        Box::pin(async move {
            // The request facade carries its endpoint scope, never an ambient writer.
            self.check_required("settings:write")?;
            let mut check = || {
                self.check_required("settings:write")
                    .map_err(|_| "Current speech settings authority refused".into())
            };
            crate::settings::update_checked(&self.state, &json!({"patch":patch}), &mut check)
                .map(|_| ())
                .map_err(|cause| speech::Error::Operation { operation, cause })
        })
    }
}
pub(crate) fn parse_unload(value: &str) -> Option<speech::Unload> {
    Some(match value {
        "never" => speech::Unload::Never,
        "immediately" => speech::Unload::Immediately,
        "min_2" => speech::Unload::Min2,
        "min_5" => speech::Unload::Min5,
        "min_10" => speech::Unload::Min10,
        "min_15" => speech::Unload::Min15,
        "hour_1" => speech::Unload::Hour1,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use speech::Settings;
    use std::{fs, os::unix::fs::DirBuilderExt};
    fn fixture(scopes: &[&str]) -> (Arc<State>, Authority) {
        crate::test_sandbox::assert_isolated();
        let root = std::env::temp_dir().join(format!(
            "speech-authority-{}",
            crate::store::random_id().unwrap()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let state = Arc::new(State::open(&root).unwrap());
        crate::settings::update(&state, &json!({"patch":{"speechAcceleration":"cpu"}})).unwrap();
        let token = state
            .issue_session(
                "private-speech-authority",
                &scopes.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
                3_600_000,
                "private",
            )
            .unwrap();
        let session = state.session(&token).unwrap();
        let authority = Authority::new(state.clone(), session, "orchestration:operate");
        (state, authority)
    }
    #[tokio::test]
    async fn bound_operate_scope_never_supplies_a_model_settings_writer() {
        let (state, authority) = fixture(&["orchestration:operate"]);
        let before = fs::read(state.dir.join("settings.json")).unwrap();
        assert!(authority
            .patch("acceleration update", json!({"speechAcceleration":"auto"}))
            .await
            .is_err());
        assert_eq!(authority.denied(), Some(Denied::Scope("settings:write")));
        assert_eq!(fs::read(state.dir.join("settings.json")).unwrap(), before);
    }
    #[test]
    fn checked_patch_revalidates_the_current_row_under_the_writer_lock() {
        let (state, authority) = fixture(&["orchestration:operate", "settings:write"]);
        let before = fs::read(state.dir.join("settings.json")).unwrap();
        let mut calls = 0;
        let mut check = || {
            calls += 1;
            assert!(state.settings_lock.try_lock().is_err());
            state
                .db
                .lock()
                .unwrap()
                .execute(
                    "UPDATE auth_sessions SET scopes='orchestration:read' WHERE session_id=?",
                    rusqlite::params![authority.session["sid"].as_str().unwrap()],
                )
                .unwrap();
            authority
                .check_required("settings:write")
                .map_err(|_| "Current speech settings authority refused".into())
        };
        assert!(crate::settings::update_checked(
            &state,
            &json!({"patch":{"speechAcceleration":"auto"}}),
            &mut check
        )
        .is_err());
        assert_eq!(calls, 1);
        assert_eq!(authority.denied(), Some(Denied::Scope("settings:write")));
        assert_eq!(fs::read(state.dir.join("settings.json")).unwrap(), before);
    }
}
