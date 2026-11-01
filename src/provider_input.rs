//! Materialize uploaded attachments from the server store, never a client supplied path.
use crate::provider::RunInput;
use base64::Engine;
use serde_json::{json, Value};

pub(crate) fn content(input: &RunInput, claude: bool) -> Result<Vec<Value>, String> {
    let mut text = input.text.clone();
    if let Some(context) = input.context.as_ref().filter(|c| !c.is_null()) {
        text.push_str("\n\nAdditional context supplied by the user:\n");
        text.push_str(&serde_json::to_string(context).map_err(|e| e.to_string())?);
    }
    let mut out = vec![json!({"type":"text","text":text})];
    for a in input.attachments.as_array().into_iter().flatten() {
        let id = a["id"].as_str().ok_or("attachment id required")?;
        let path = crate::assets::attachment_path_in(&input.attachment_root, id)
            .ok_or("uploaded attachment is missing")?;
        let name = a["name"].as_str().unwrap_or(id);
        let mime = a["mimeType"].as_str().unwrap_or("application/octet-stream");
        if matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            if claude {
                // Keep the transport bounded, including base64 expansion.
                let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                if file.metadata().map_err(|e| e.to_string())?.len() > 5 * 1024 * 1024 {
                    return Err("Claude image exceeds 5 MiB".into());
                }
                use std::io::Read;
                let mut bytes = Vec::new();
                file.take(5 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 5 * 1024 * 1024 {
                    return Err("Claude image exceeds 5 MiB".into());
                }
                out.push(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}}));
            } else {
                out.push(json!({"type":"localImage","path":path}));
            }
        } else {
            // Both native agents can read arbitrary file formats with their own tools.
            out.push(json!({"type":"text","text":format!("User attached file {} ({mime}), available at {}. Read it when needed.", serde_json::to_string(name).unwrap(), serde_json::to_string(&path).unwrap())}));
        }
    }
    if serde_json::to_vec(&out).map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
        return Err("provider input exceeds 8 MiB".into());
    }
    Ok(out)
}
