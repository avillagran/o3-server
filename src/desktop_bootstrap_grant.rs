//! Process-local desktop grants, deliberately separate from persisted one-time pairing links.
use crate::desktop_bootstrap::{valid_token, LEGACY_TTL_MS, WINDOW_MS};

pub struct Credentials {
    secret: Option<String>,
    legacy: Option<(String, i64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidKey,
    Expired,
    ScopeNotGranted,
}

pub struct Grant {
    pub expires_at: i64,
}

impl Credentials {
    pub fn new(token: Option<String>, secret: Option<String>, now_ms: i64) -> Self {
        let legacy = token
            .filter(|token| !token.is_empty() && secret.is_none())
            .map(|token| (token, now_ms + LEGACY_TTL_MS));
        Self { secret, legacy }
    }

    /// None delegates to the ordinary pairing repository. Successful desktop grants are reusable.
    pub fn consume(
        &mut self,
        credential: &str,
        now_ms: i64,
        requested_scopes: Option<&[String]>,
        required_scopes: Option<&[String]>,
    ) -> Result<Option<Grant>, Error> {
        if self
            .secret
            .as_deref()
            .map(|secret| valid_token(secret, credential, now_ms))
            .transpose()
            .map_err(|_| Error::InvalidKey)?
            .unwrap_or(false)
        {
            // Native rotating grants precede seeded-grant scope filtering. The exchange
            // service still intersects the returned grant with the requested OAuth scopes.
            return Ok(Some(Grant {
                expires_at: now_ms + WINDOW_MS,
            }));
        }
        let Some((token, expires_at)) = &self.legacy else {
            return Ok(None);
        };
        if credential != token {
            return Ok(None);
        }
        if now_ms >= *expires_at {
            self.legacy = None;
            return Err(Error::Expired);
        }
        let granted = |scope: &String| crate::store::STANDARD_SCOPES.contains(&scope.as_str());
        if requested_scopes.is_some_and(|scopes| !scopes.iter().any(granted))
            || required_scopes.is_some_and(|scopes| !scopes.iter().all(granted))
        {
            return Err(Error::ScopeNotGranted);
        }
        Ok(Some(Grant {
            expires_at: *expires_at,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    #[test]
    fn native_grant_store_oracle() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/desktop-grants-4288.json"))
                .unwrap();
        assert_eq!(
            json!(crate::store::STANDARD_SCOPES),
            fixture["standardScopes"]
        );
        for case in fixture["cases"].as_array().unwrap() {
            let input = &case["input"];
            let configuration = &input["configuration"];
            let mut credentials = Credentials::new(
                configuration["desktopBootstrapToken"]
                    .as_str()
                    .map(str::to_owned),
                configuration["desktopBootstrapSecret"]
                    .as_str()
                    .map(str::to_owned),
                0,
            );
            let now = input["elapsed"].as_i64().unwrap();
            let kind = input["credentialKind"].as_str().unwrap();
            let offset = match kind {
                "current" => Some(0),
                "previous" => Some(-1),
                "too-old" => Some(-2),
                _ => None,
            };
            let credential = offset
                .map(|offset| {
                    crate::desktop_bootstrap::token_at("rotating", now + offset * WINDOW_MS)
                        .unwrap()
                })
                .unwrap_or_else(|| kind.into());
            let requested: Option<Vec<String>> = input
                .get("requestedScopes")
                .map(|value| serde_json::from_value(value.clone()).unwrap());
            let required: Option<Vec<String>> = input
                .get("requiredScopes")
                .map(|value| serde_json::from_value(value.clone()).unwrap());
            let mut results = Vec::new();
            let mut repository_calls = Vec::new();
            let mut invalid_key = false;
            for _ in 0..2 {
                let result = credentials.consume(
                    &credential,
                    now,
                    requested.as_deref(),
                    required.as_deref(),
                );
                results.push(match result {
                    Ok(Some(grant)) => json!({"method":"desktop-bootstrap","subject":"desktop-bootstrap","scopes":crate::store::STANDARD_SCOPES,"expiresAt":grant.expires_at}),
                    Ok(None) => {repository_calls.push(credential.clone()); json!({"error":"UnknownBootstrapCredentialError"})},
                    Err(Error::Expired) => json!({"error":"ExpiredBootstrapCredentialError"}),
                    Err(Error::ScopeNotGranted) => json!({"error":"BootstrapCredentialScopeNotGrantedError"}),
                    Err(Error::InvalidKey) => {invalid_key=true; break;},
                });
            }
            let actual = if invalid_key {
                json!({"error":"InvalidKey"})
            } else {
                json!({"credential":credential,"results":results})
            };
            assert_eq!(actual, case["expected"], "{input}");
            assert_eq!(json!(repository_calls), case["repositoryCalls"], "{input}");
        }
    }
}
