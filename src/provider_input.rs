//! Materialize uploaded attachments from the server store, never a client supplied path.
use crate::provider::RunInput;
use base64::Engine;
use serde_json::{json, Value};

pub(crate) fn content(input: &RunInput, claude: bool) -> Result<Vec<Value>, String> {
    materialize(
        &input.attachment_root,
        &input.text,
        &input.attachments,
        input.context.as_ref(),
        claude,
    )
}

pub(crate) fn materialize(
    root: &std::path::Path,
    text: &str,
    attachments: &Value,
    context: Option<&Value>,
    claude: bool,
) -> Result<Vec<Value>, String> {
    let mut text = text.to_owned();
    if let Some(context) = context.filter(|c| !c.is_null()) {
        text.push_str("\n\nAdditional context supplied by the user:\n");
        text.push_str(&serde_json::to_string(context).map_err(|e| e.to_string())?);
    }
    let mut budget = text.len().saturating_add(1024);
    let mut out = vec![json!({"type":"text","text":text})];
    for a in attachments.as_array().into_iter().flatten() {
        let id = a["id"].as_str().ok_or("attachment id required")?;
        let path =
            crate::assets::attachment_path_in(root, id).ok_or("uploaded attachment is missing")?;
        let name = a["name"].as_str().unwrap_or(id);
        let mime = a["mimeType"].as_str().unwrap_or("application/octet-stream");
        if matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) {
            if claude {
                // Only Claude carries image bytes in the frame; Codex gets a path.
                let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                budget = budget.saturating_add(
                    usize::try_from(size)
                        .unwrap_or(usize::MAX)
                        .saturating_mul(4)
                        / 3
                        + 1024,
                );
                if budget > 8 * 1024 * 1024 {
                    return Err("provider images exceed input budget".into());
                }
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

/// Resolve configured native drivers as well as the built-in instance IDs.
pub(crate) fn driver(s: &crate::store::State, instance: &Value) -> Result<String, String> {
    let id = instance.as_str().ok_or("provider instance required")?;
    let settings = crate::settings::read(s)?;
    Ok(settings["providerInstances"][id]["driver"]
        .as_str()
        .unwrap_or(if id == "codex" {
            "codex"
        } else {
            "claudeAgent"
        })
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_paths_do_not_pay_for_base64_that_is_never_sent() {
        let root = std::env::temp_dir().join(format!("o3-input-{}", std::process::id()));
        let id = "upload-11111111-1111-4111-8111-111111111111";
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(format!("{id}.png")), vec![0u8; 7 * 1024 * 1024]).unwrap();
        let attachments = json!([{"id":id,"name":"big.png","mimeType":"image/png"}]);
        let codex = materialize(&root, "look", &attachments, None, false);
        let claude = materialize(&root, "look", &attachments, None, true);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(codex.unwrap().iter().any(|v| v["type"] == "localImage"));
        assert!(claude.is_err());
    }
}
