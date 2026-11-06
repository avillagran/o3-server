//! Closed interpreter for the complete pinned native ServerSettings schema.
//! The program is exported from Effect's AST, including every transform and refinement.
use serde_json::{json, Map, Value};
use std::{collections::HashSet, sync::LazyLock};
use unicode_normalization::UnicodeNormalization;

static PROGRAM: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("managed-settings-codec-4262.json"))
        .expect("pinned settings codec")
});
type Result<T> = std::result::Result<T, ()>;

pub(crate) fn is_secret_reference(value: &Value) -> bool {
    run(
        id(&PROGRAM["secretReference"]),
        Some(value.clone()),
        true,
        false,
    )
    .is_ok()
}
pub(crate) fn secret_fields() -> &'static Value {
    &PROGRAM["secretFields"]
}
pub(crate) fn defaults() -> Value {
    PROGRAM["defaults"].clone()
}
pub(crate) fn encoded_defaults() -> Value {
    PROGRAM["encodedDefaults"].clone()
}
pub(crate) fn fields() -> &'static Map<String, Value> {
    PROGRAM["fields"].as_object().unwrap()
}
pub(crate) fn project_override_keys() -> &'static Vec<Value> {
    PROGRAM["projectOverrideKeys"].as_array().unwrap()
}
pub(crate) fn decode_persisted(value: Value) -> Result<Value> {
    run(id(&PROGRAM["persistedRoot"]), Some(value), false, false)?.ok_or(())
}
pub(crate) fn encode_sparse(value: Value) -> Result<Value> {
    encode(value)
}
pub(crate) fn decode_patch(value: Value) -> Result<Value> {
    run(id(&PROGRAM["patchRoot"]), Some(value), false, false)?.ok_or(())
}
pub(crate) fn decode(value: Value) -> Result<Value> {
    run(
        PROGRAM["root"].as_u64().unwrap() as usize,
        Some(value),
        false,
        false,
    )?
    .ok_or(())
}
pub(crate) fn encode(value: Value) -> Result<Value> {
    run(
        PROGRAM["root"].as_u64().unwrap() as usize,
        Some(value),
        true,
        false,
    )?
    .ok_or(())
}
fn node(id: usize) -> &'static Value {
    &PROGRAM["nodes"][id]
}
fn id(value: &Value) -> usize {
    value.as_u64().unwrap() as usize
}
pub(crate) fn array_index(key: &str) -> Option<u32> {
    let value = key.parse::<u32>().ok()?;
    (value != u32::MAX && value.to_string() == key).then_some(value)
}
fn entries(value: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut result: Vec<_> = value.iter().collect();
    result.sort_by_key(|(k, _)| array_index(k).map_or((1, 0), |v| (0, v)));
    result
}
fn js_space(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn trim(s: &str) -> &str {
    s.trim_matches(js_space)
}
fn optional(id_: usize) -> bool {
    let n = node(id_);
    n["optional"] == true
        || n.get("codecs")
            .and_then(Value::as_array)
            .and_then(|c| c.last())
            .is_some_and(|c| optional(id(&c["from"])))
}
fn run(index: usize, mut value: Option<Value>, encode: bool, bare: bool) -> Result<Option<Value>> {
    let n = node(index);
    if value.is_none() && n["optional"] == true {
        return Ok(None);
    }
    if let Some(codecs) = n["codecs"].as_array().filter(|_| !bare) {
        if encode {
            value = run(index, value, true, true)?;
            for codec in codecs {
                value = transform(index, codec["transform"].as_str().unwrap(), value, true)?;
                value = run(id(&codec["from"]), value, true, false)?;
            }
        } else {
            for codec in codecs.iter().rev() {
                value = run(id(&codec["from"]), value, false, false)?;
                value = transform(index, codec["transform"].as_str().unwrap(), value, false)?;
            }
            value = run(index, value, false, true)?;
        }
        return Ok(value);
    }
    if value.is_none() && n["optional"] == true {
        return Ok(None);
    }
    let result = match n["tag"].as_str().unwrap() {
        "Unknown" => value,
        "Undefined" => {
            if value.is_some() {
                return Err(());
            }
            None
        }
        "String" => {
            if !value.as_ref().is_some_and(Value::is_string) {
                return Err(());
            }
            value
        }
        "Number" | "Declaration" => {
            if !value.as_ref().is_some_and(Value::is_number) {
                return Err(());
            }
            value
        }
        "Boolean" => {
            if !value.as_ref().is_some_and(Value::is_boolean) {
                return Err(());
            }
            value
        }
        "Null" => {
            if !value.as_ref().is_some_and(Value::is_null) {
                return Err(());
            }
            value
        }
        "Literal" => {
            if value.as_ref() != Some(&n["literal"]) {
                return Err(());
            }
            value
        }
        "Union" => {
            let mut found = None;
            for t in n["types"].as_array().unwrap() {
                if let Ok(v) = run(id(t), value.clone(), encode, false) {
                    found = Some(v);
                    break;
                }
            }
            found.ok_or(())?
        }
        "Arrays" => {
            let input = value.as_ref().and_then(Value::as_array).ok_or(())?;
            let elements = n["elements"].as_array().unwrap();
            let rest = n["rest"].as_array().unwrap();
            if input.len() < elements.len() || (rest.is_empty() && input.len() > elements.len()) {
                return Err(());
            }
            let mut output = Vec::new();
            for (i, v) in input.iter().enumerate() {
                let t = elements.get(i).or_else(|| rest.first()).ok_or(())?;
                output.push(run(id(t), Some(v.clone()), encode, false)?.ok_or(())?)
            }
            Some(Value::Array(output))
        }
        "Objects" => {
            let input = value.as_ref().and_then(Value::as_object).ok_or(())?;
            let mut output = Map::new();
            let mut known = HashSet::new();
            for p in n["properties"].as_array().unwrap() {
                let key = p["name"].as_str().unwrap();
                known.insert(key);
                let t = id(&p["type"]);
                let v = input.get(key).cloned();
                if encode && v.is_none() && optional(t) {
                    continue;
                }
                if v.is_none() && !optional(t) {
                    return Err(());
                }
                if let Some(v) = run(t, v, encode, false)? {
                    output.insert(key.to_owned(), v);
                }
            }
            for (key, v) in entries(input) {
                if known.contains(key.as_str()) {
                    continue;
                }
                for ix in n["indexes"].as_array().unwrap() {
                    if let Ok(Some(k)) = run(
                        id(&ix["key"]),
                        Some(Value::String(key.clone())),
                        encode,
                        false,
                    ) {
                        let k = k.as_str().ok_or(())?;
                        if let Some(v) = run(id(&ix["value"]), Some(v.clone()), encode, false)? {
                            output.insert(k.to_owned(), v);
                        }
                    }
                }
            }
            Some(Value::Object(output))
        }
        _ => return Err(()),
    };
    if let Some(v) = &result {
        for check in n["checks"].as_array().into_iter().flatten() {
            check_value(check, v)?;
        }
    }
    Ok(result)
}
fn transform(
    index: usize,
    kind: &str,
    value: Option<Value>,
    encode: bool,
) -> Result<Option<Value>> {
    let n = node(index);
    match kind {
        "default" => Ok(if !encode && value.is_none() {
            Some(n["default"].clone())
        } else {
            value
        }),
        "retiredStream" => Ok(value.map(|v| {
            if !encode && v == "token" {
                json!("paragraph")
            } else {
                v
            }
        })),
        "identity" | "durationMillis" => Ok(value),
        "trim" => Ok(Some(Value::String(
            trim(value.as_ref().and_then(Value::as_str).ok_or(())?).to_owned(),
        ))),
        "safeNullable" | "safeOptional" | "safeNullableKey" => {
            if encode {
                return Ok(
                    if kind == "safeNullableKey" && value.as_ref().is_none_or(Value::is_null) {
                        None
                    } else {
                        value
                    },
                );
            }
            if run(index, value.clone(), false, true).is_ok() {
                Ok(value)
            } else if kind == "safeOptional" {
                Ok(None)
            } else {
                Ok(Some(Value::Null))
            }
        }
        "modelSelection" => {
            let mut input = value.and_then(|v| v.as_object().cloned()).ok_or(())?;
            if !encode
                && !input.contains_key("instanceId")
                && input.get("provider").is_some_and(Value::is_string)
            {
                input.insert("instanceId".to_owned(), input["provider"].clone());
            }
            input.remove("provider");
            Ok(Some(Value::Object(input)))
        }
        "legacyOptions" => {
            if encode {
                let mut out = Map::new();
                for v in value.as_ref().and_then(Value::as_array).ok_or(())? {
                    out.insert(v["id"].as_str().ok_or(())?.to_owned(), v["value"].clone());
                }
                Ok(Some(Value::Object(out)))
            } else {
                let mut out = Vec::new();
                for (k, v) in entries(value.as_ref().and_then(Value::as_object).ok_or(())?) {
                    let k = trim(k);
                    if k.is_empty() {
                        continue;
                    }
                    let v = if let Some(v) = v.as_str() {
                        let v = trim(v);
                        if v.is_empty() {
                            continue;
                        }
                        Value::String(v.to_owned())
                    } else if v.is_boolean() {
                        v.clone()
                    } else {
                        continue;
                    };
                    out.push(json!({"id":k,"value":v}));
                }
                Ok(Some(Value::Array(out)))
            }
        }
        "legacyWritingStyle" => {
            if encode {
                Ok(Some(value.ok_or(())?["customInstructions"].clone()))
            } else {
                let s = value.as_ref().and_then(Value::as_str).ok_or(())?;
                Ok(Some(
                    json!({"mode":if s.is_empty(){"repo_conventions"}else{"custom"},"customInstructions":s,"followChangeRequestTemplates":true}),
                ))
            }
        }
        "stringArray" => {
            if encode {
                Ok(value)
            } else {
                Ok(Some(Value::Array(
                    value
                        .and_then(|v| v.as_array().cloned())
                        .ok_or(())?
                        .into_iter()
                        .filter(Value::is_string)
                        .collect(),
                )))
            }
        }
        "environmentSecret" => {
            let mut v = value.and_then(|v| v.as_object().cloned()).ok_or(())?;
            if encode {
                if v.get("value").is_some_and(|v| !v.is_string()) {
                    let secret = v.insert("value".to_owned(), json!("")).unwrap();
                    v.insert("secret".to_owned(), secret);
                }
            } else if let Some(secret) = v.remove("secret") {
                v.insert("value".to_owned(), secret);
                v.insert("sensitive".to_owned(), json!(true));
            }
            Ok(Some(Value::Object(v)))
        }
        _ => Err(()),
    }
}
fn check_value(check: &Value, v: &Value) -> Result<()> {
    let length = || {
        v.as_str()
            .map(|s| s.encode_utf16().count())
            .or_else(|| v.as_array().map(Vec::len))
    };
    let number = || v.as_f64().ok_or(());
    let valid = match check["kind"].as_str().unwrap() {
        "nonBlank" => v.as_str().is_some_and(|s| !trim(s).is_empty()),
        "isMaxLength" => {
            length().is_some_and(|n| n <= check["maxLength"].as_u64().unwrap() as usize)
        }
        "isMinLength" => {
            length().is_some_and(|n| n >= check["minLength"].as_u64().unwrap() as usize)
        }
        "isInt" => {
            let x = number()?;
            x.fract() == 0. && x.abs() <= 9007199254740991.
        }
        "isFinite" => number()?.is_finite(),
        "isBetween" => {
            let x = number()?;
            x >= check["minimum"].as_f64().unwrap() && x <= check["maximum"].as_f64().unwrap()
        }
        "isGreaterThanOrEqualTo" => number()? >= check["minimum"].as_f64().unwrap(),
        "isPattern" => {
            let pattern=check["source"].as_str().unwrap().replace("\\/","/").replace("\\d","[0-9]").replace("\\s", "\\x{0009}-\\x{000D}\\x{0020}\\x{00A0}\\x{1680}\\x{2000}-\\x{200A}\\x{2028}\\x{2029}\\x{202F}\\x{205F}\\x{3000}\\x{FEFF}");
            let pattern = pattern.replace(".+", r"[^\n\r\x{2028}\x{2029}]+");
            regex::Regex::new(&pattern)
                .map_err(|_| ())?
                .is_match(v.as_str().ok_or(())?)
        }
        "configuredHostId" => v.as_str().is_some_and(|s| s != "local"),
        "uniqueHostIds" => {
            let mut ids = HashSet::new();
            v.as_array()
                .ok_or(())?
                .iter()
                .all(|v| ids.insert(v["id"].clone().to_string()))
        }
        "easBounds" => {
            v["idleMinutes"].as_f64().ok_or(())? < v["maxSessionMinutes"].as_f64().ok_or(())?
        }
        "speechWords" => {
            let mut words = HashSet::new();
            let mut valid = true;
            for entry in v.as_array().ok_or(())? {
                for word in std::iter::once(&entry["term"])
                    .chain(entry["aliases"].as_array().into_iter().flatten())
                {
                    let word = trim(word.as_str().ok_or(())?)
                        .nfc()
                        .collect::<String>()
                        .to_lowercase();
                    if word.is_empty() || !words.insert(word) {
                        valid = false
                    }
                }
            }
            valid
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_sparse_encoding() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/managed-settings-sparse-4262.json"
        ))
        .unwrap();
        for case in fixture.as_array().unwrap() {
            assert_eq!(
                encode_sparse(case["input"].clone()),
                Ok(case["encoded"].clone()),
                "sparse {case}"
            );
        }
        let mut defaults = encoded_defaults();
        defaults["responseStreamingMode"] = json!("token");
        defaults["projectSettingsOverrides"] =
            json!({"project":{"responseStreamingMode":"token","future":1}});
        let decoded = decode_persisted(defaults).unwrap();
        assert_eq!(decoded["responseStreamingMode"], "paragraph");
        assert_eq!(
            decoded["projectSettingsOverrides"]["project"],
            json!({"responseStreamingMode":"paragraph","future":1})
        );
    }
    #[test]
    fn pinned_every_ast_node() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/managed-settings-nodes-4262.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let index = id(&case["node"]);
            let input = fixture["values"].get(id(&case["valueIndex"])).cloned();
            let result = if input.is_none() && !optional(index) {
                Err(())
            } else {
                run(index, input, false, false)
            };
            if case["invalid"] == true {
                assert!(
                    result.is_err(),
                    "node {index} case {case}: unexpectedly valid {result:?}"
                );
                continue;
            }
            assert_eq!(
                result,
                Ok(case["decoded"].get("value").cloned()),
                "node {index} case {case}: decode"
            );
            let encoded = run(index, result.unwrap(), true, false);
            assert_eq!(
                encoded,
                Ok(case["encoded"].get("value").cloned()),
                "node {index} case {case}: encode"
            );
        }
    }
    #[test]
    fn pinned_full_schema_codec() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/managed-settings-codec-4262.json"
        ))
        .unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let label = case["label"].as_str().unwrap();
            if case["empty"] == true {
                assert_eq!(decode(json!({})), Ok(defaults()), "{label}");
                continue;
            }
            let key = case["key"].as_str().unwrap();
            let mut input = encoded_defaults();
            if case["missing"] == true {
                input.as_object_mut().unwrap().remove(key);
            } else {
                input[key] = case["value"].clone();
            }
            let result = decode(input);
            if case["invalid"] == true {
                assert!(result.is_err(), "{label}: unexpectedly valid {result:?}");
                continue;
            }
            let result = result.unwrap_or_else(|()| panic!("{label}: unexpectedly invalid"));
            assert_eq!(
                result.get(key),
                case["decoded"].get("value"),
                "{label}: decoded"
            );
            let encoded = encode(result).unwrap_or_else(|()| panic!("{label}: encode failed"));
            assert_eq!(
                encoded.get(key),
                case["encoded"].get("value"),
                "{label}: encoded"
            );
        }
    }
}
