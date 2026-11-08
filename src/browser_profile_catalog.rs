//! Pinned native browser profile reports. Tokens belong to actual connection
//! lifetimes; profile partitions are never inferred from files or open tabs.
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, Weak};

const DEFAULT: &str = "default";
const INCOGNITO: &str = "incognito";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Persistent,
    Incognito,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Profile {
    id: String,
    name: String,
    kind: Kind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    profiles: Vec<Profile>,
    default_profile_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    recording_frame_rate: Option<u8>,
}
fn text(
    value: Option<&Value>,
    name: &str,
    limit: usize,
    identifier: bool,
) -> Result<String, String> {
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} must be a string"))?;
    let text = crate::native_goal::trim(text);
    if text.is_empty()
        || text.encode_utf16().count() > limit
        || identifier && text.chars().any(char::is_control)
    {
        return Err(format!("{name} is not a valid browser profile value"));
    }
    Ok(text.to_owned())
}
impl Report {
    /// Matches the original decoded schema for scalar Unicode strings, strips
    /// unknown fields, and preserves profile array order and duplicate IDs.
    pub fn decode(input: &Value) -> Result<Self, String> {
        let input = input
            .as_object()
            .ok_or("Expected browser profile report object")?;
        let profiles = input
            .get("profiles")
            .and_then(Value::as_array)
            .ok_or("profiles must be an array")?;
        if profiles.len() > 24 {
            return Err("profiles may contain at most 24 entries".into());
        }
        let mut decoded = Vec::with_capacity(profiles.len());
        for profile in profiles {
            let profile = profile
                .as_object()
                .ok_or("Expected browser profile object")?;
            let kind = match profile.get("kind").and_then(Value::as_str) {
                Some("persistent") => Kind::Persistent,
                Some("incognito") => Kind::Incognito,
                _ => return Err("Browser profile kind must be persistent or incognito".into()),
            };
            decoded.push(Profile {
                id: text(profile.get("id"), "id", 64, true)?,
                name: text(profile.get("name"), "name", 48, false)?,
                kind,
            });
        }
        let recording_frame_rate = match input.get("recordingFrameRate") {
            None => None,
            Some(value) if value.as_f64() == Some(30.0) => Some(30),
            Some(value) if value.as_f64() == Some(60.0) => Some(60),
            Some(_) => return Err("recordingFrameRate must be 30 or 60".into()),
        };
        Ok(Self {
            profiles: decoded,
            default_profile_id: text(input.get("defaultProfileId"), "defaultProfileId", 64, true)?,
            recording_frame_rate,
        })
    }
    pub fn value(&self) -> Value {
        serde_json::to_value(self).expect("Report serialization is infallible")
    }
}
fn builtins() -> Vec<Profile> {
    vec![
        Profile {
            id: DEFAULT.into(),
            name: "Default".into(),
            kind: Kind::Persistent,
        },
        Profile {
            id: INCOGNITO.into(),
            name: "Incognito".into(),
            kind: Kind::Incognito,
        },
    ]
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub profiles: Vec<Profile>,
    pub default_profile_id: Option<String>,
    pub recording_frame_rate: u8,
}
impl Snapshot {
    /// Raw actual catalog for the pinned original profileStatus closure.
    pub(crate) fn source_status_catalog(&self) -> Value {
        let mut value = json!({"profiles": self.profiles});
        if let Some(default) = &self.default_profile_id {
            value["defaultProfileId"] = json!(default);
        }
        value
    }

    /// Original status projection: omit the default until an actual report;
    /// emit `incognito` only for the built-in incognito partition.
    pub fn status(&self) -> Value {
        let mut value = json!({"profiles":self.profiles.iter().map(|profile| {
            let mut value=json!({"id":profile.id,"name":profile.name});
            if profile.kind==Kind::Incognito {value["incognito"]=json!(true);}
            value
        }).collect::<Vec<_>>()});
        if let Some(default) = &self.default_profile_id {
            value["defaultProfileId"] = json!(default);
        }
        value
    }
}
#[derive(Default)]
struct Inner {
    next: u64,
    reports: Vec<(u64, Report)>,
    reported: Option<(Vec<Profile>, String)>,
    recording_frame_rate: Option<u8>,
}
impl Inner {
    fn rebuild(&mut self) {
        let Some((_, latest)) = self.reports.last() else {
            return;
        };
        // JS Map replacement retains its first insertion position. A reporter
        // update, however, deletes and reinserts that reporter at the end.
        let mut merged: Vec<Profile> = Vec::new();
        for (_, report) in &self.reports {
            for profile in &report.profiles {
                if let Some(existing) = merged
                    .iter_mut()
                    .find(|candidate| candidate.id == profile.id)
                {
                    *existing = profile.clone();
                } else {
                    merged.push(profile.clone());
                }
            }
        }
        let mut profiles = builtins();
        for mut profile in merged {
            if profiles.iter().any(|candidate| candidate.id == profile.id) {
                continue;
            }
            profile.kind = Kind::Persistent;
            profiles.push(profile);
        }
        let default = if profiles
            .iter()
            .any(|profile| profile.id == latest.default_profile_id)
        {
            latest.default_profile_id.clone()
        } else {
            DEFAULT.into()
        };
        self.reported = Some((profiles, default));
    }
    fn snapshot(&self) -> Snapshot {
        let (profiles, default_profile_id) = self.reported.as_ref().map_or_else(
            || (builtins(), None),
            |(profiles, default)| (profiles.clone(), Some(default.clone())),
        );
        Snapshot {
            profiles,
            default_profile_id,
            recording_frame_rate: self.recording_frame_rate.unwrap_or(30),
        }
    }
}
#[derive(Clone, Default)]
pub struct Catalog(Arc<Mutex<Inner>>);
impl Catalog {
    pub fn reporter(&self) -> Result<Reporter, String> {
        let mut inner = self.0.lock().unwrap();
        let id = inner.next;
        inner.next = inner
            .next
            .checked_add(1)
            .ok_or("Browser profile reporter identity exhausted")?;
        Ok(Reporter {
            catalog: Arc::downgrade(&self.0),
            id,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.0.lock().unwrap().snapshot()
    }
}
/// Non-cloneable, unforgeable reporter for one actual connection lifetime.
/// Its drop retires only its in-memory report; no filesystem cleanup occurs.
pub struct Reporter {
    catalog: Weak<Mutex<Inner>>,
    id: u64,
}
impl Reporter {
    pub fn report(&self, input: Report) -> Result<(), String> {
        let catalog = self
            .catalog
            .upgrade()
            .ok_or("Browser profile coordinator closed")?;
        let mut inner = catalog.lock().unwrap();
        if let Some(rate) = input.recording_frame_rate {
            inner.recording_frame_rate = Some(rate);
        }
        inner.reports.retain(|(id, _)| *id != self.id);
        inner.reports.push((self.id, input));
        inner.rebuild();
        Ok(())
    }
}
impl Drop for Reporter {
    fn drop(&mut self) {
        if let Some(catalog) = self.catalog.upgrade() {
            let mut inner = catalog.lock().unwrap();
            inner.reports.retain(|(id, _)| *id != self.id);
            inner.rebuild();
        }
    }
}

#[cfg(test)]
#[path = "browser_profile_catalog_native_tests.rs"]
mod native_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn report(profiles: Value, default: &str, rate: Option<u8>) -> Report {
        let mut value = json!({"profiles":profiles,"defaultProfileId":default});
        if let Some(rate) = rate {
            value["recordingFrameRate"] = json!(rate);
        }
        Report::decode(&value).unwrap()
    }
    fn profile(id: &str, name: &str) -> Value {
        json!({"id":id,"name":name,"kind":"persistent"})
    }
    #[test]
    fn unreported_catalog_has_real_builtins_but_no_reported_default() {
        crate::test_sandbox::assert_isolated();
        let catalog = Catalog::default();
        let token = catalog.reporter().unwrap();
        assert_eq!(
            catalog.snapshot().status(),
            json!({"profiles":[{"id":"default","name":"Default"},{"id":"incognito","name":"Incognito","incognito":true}]})
        );
        assert_eq!(catalog.snapshot().recording_frame_rate, 30);
        drop(token);
        assert!(catalog.snapshot().default_profile_id.is_none());
    }
    #[test]
    fn union_replacement_and_reporter_update_preserve_native_map_order() {
        crate::test_sandbox::assert_isolated();
        let catalog = Catalog::default();
        let a = catalog.reporter().unwrap();
        let b = catalog.reporter().unwrap();
        a.report(report(
            json!([profile("a", "A"), profile("x", "Old X")]),
            "a",
            Some(60),
        ))
        .unwrap();
        b.report(report(
            json!([profile("b", "B"), profile("x", "New X")]),
            "b",
            None,
        ))
        .unwrap();
        let status = catalog.snapshot().status();
        assert_eq!(
            status["profiles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["default", "incognito", "a", "x", "b"]
        );
        assert_eq!(status["profiles"][3]["name"], "New X");
        assert_eq!(status["defaultProfileId"], "b");
        a.report(report(
            json!([profile("c", "C"), profile("x", "Latest X")]),
            "c",
            None,
        ))
        .unwrap();
        let snapshot = catalog.snapshot();
        let status = snapshot.status();
        assert_eq!(
            status["profiles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["default", "incognito", "b", "x", "c"]
        );
        assert_eq!(status["profiles"][3]["name"], "Latest X");
        assert_eq!(status["defaultProfileId"], "c");
        assert_eq!(
            snapshot.recording_frame_rate, 60,
            "Omitting rate retains its last global value"
        );
    }
    #[test]
    fn retiring_reports_rebuilds_union_then_retains_last_catalog_and_rate() {
        crate::test_sandbox::assert_isolated();
        let catalog = Catalog::default();
        let a = catalog.reporter().unwrap();
        let b = catalog.reporter().unwrap();
        a.report(report(json!([profile("a", "A")]), "a", Some(60)))
            .unwrap();
        b.report(report(json!([profile("b", "B")]), "b", Some(30)))
            .unwrap();
        drop(b);
        let last = catalog.snapshot();
        assert_eq!(last.default_profile_id, Some("a".into()));
        assert_eq!(
            last.recording_frame_rate, 30,
            "Retiring latest reporter does not restore its predecessor's rate"
        );
        drop(a);
        assert_eq!(catalog.snapshot(), last);
        let empty = catalog.reporter().unwrap();
        empty.report(report(json!([]), "missing", None)).unwrap();
        assert_eq!(catalog.snapshot().profiles, builtins());
        assert_eq!(catalog.snapshot().default_profile_id, Some(DEFAULT.into()));
        drop(empty);
        assert_eq!(catalog.snapshot().profiles, builtins());
    }
    #[test]
    fn builtins_cannot_be_overridden_and_only_builtin_incognito_is_ephemeral() {
        crate::test_sandbox::assert_isolated();
        let catalog = Catalog::default();
        let token = catalog.reporter().unwrap();
        token
            .report(report(
                json!([
                    {"id":"incognito","name":"Wrong","kind":"persistent"},
                    {"id":"custom","name":"Private label","kind":"incognito"},
                    profile("custom","Last duplicate"),
                    profile("default","Wrong default")
                ]),
                "CUSTOM",
                None,
            ))
            .unwrap();
        let status = catalog.snapshot().status();
        assert_eq!(status["profiles"][0]["name"], "Default");
        assert_eq!(status["profiles"][1]["name"], "Incognito");
        assert_eq!(
            status["profiles"][2],
            json!({"id":"custom","name":"Last duplicate"})
        );
        assert_eq!(
            status["defaultProfileId"], "default",
            "Default matching is exact, not case-insensitive"
        );
    }
    #[test]
    fn decoder_uses_ecmascript_trim_utf16_limits_and_no_identifier_controls() {
        crate::test_sandbox::assert_isolated();
        let decoded=Report::decode(&json!({"profiles":[{"id":" \u{feff}x\u{feff} ","name":" \u{feff}Name ","kind":"persistent","ignored":true}],"defaultProfileId":" x ","recordingFrameRate":60.0,"ignored":1})).unwrap();
        assert_eq!(
            decoded.value(),
            json!({"profiles":[profile("x","Name")],"defaultProfileId":"x","recordingFrameRate":60})
        );
        // NEL is a control character but not ECMAScript whitespace; names allow
        // it, while profile IDs reject Unicode Cc anywhere after decoding.
        assert!(Report::decode(
            &json!({"profiles":[profile("x","\u{85}")],"defaultProfileId":"x"})
        )
        .is_ok());
        assert!(Report::decode(
            &json!({"profiles":[profile("\u{85}","Name")],"defaultProfileId":"x"})
        )
        .is_err());
        assert!(Report::decode(
            &json!({"profiles":[profile("x",&"😀".repeat(24))],"defaultProfileId":"x"})
        )
        .is_ok());
        assert!(Report::decode(
            &json!({"profiles":[profile("x",&"😀".repeat(25))],"defaultProfileId":"x"})
        )
        .is_err());
        for rate in [Value::Null, json!(29), json!(60.5), json!("30")] {
            assert!(Report::decode(
                &json!({"profiles":[],"defaultProfileId":"default","recordingFrameRate":rate})
            )
            .is_err());
        }
        assert!(Report::decode(
            &json!({"profiles":vec![profile("x","X");25],"defaultProfileId":"default"})
        )
        .is_err());
    }
    #[test]
    fn token_does_not_keep_a_closed_coordinator_alive() {
        crate::test_sandbox::assert_isolated();
        let catalog = Catalog::default();
        let token = catalog.reporter().unwrap();
        drop(catalog);
        assert!(token.report(report(json!([]), "default", None)).is_err());
    }
}
