//! Input decoding for the nightly thread-read schemas. Decode defects precede typed handlers.
use serde_json::{json, Value};

pub(crate) fn decode(tag: &str, input: &Value) -> Result<Option<Value>, String> {
    let (field, array) = match tag {
        "orchestration.getThreadTitles" => ("threadIds", true),
        "orchestration.getThreadFamily" => ("threadId", false),
        "orchestration.getWorktreeThreadIds" => ("worktreePath", false),
        _ => return Ok(None),
    };
    let object = input.as_object().ok_or("Expected object")?;
    let path = format!("[\"{field}\"]");
    let value = object
        .get(field)
        .ok_or_else(|| format!("Missing key\n  at {path}"))?;
    let string = |value: &Value, path: &str| -> Result<Value, String> {
        let text = value
            .as_str()
            .ok_or_else(|| format!("Expected string\n  at {path}"))?;
        let text = crate::native_goal::trim(text);
        if text.is_empty() {
            return Err(format!("Expected a non-blank string\n  at {path}"));
        }
        Ok(json!(text))
    };
    let value = if array {
        let values = value
            .as_array()
            .ok_or_else(|| format!("Expected array\n  at {path}"))?;
        Value::Array(
            values
                .iter()
                .enumerate()
                .map(|(i, v)| string(v, &format!("{path}[{i}]")))
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        string(value, &path)?
    };
    Ok(Some(json!({field:value})))
}
