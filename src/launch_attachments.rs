//! Pending upload claims for MCP thread launch. Copies isolate the durable message
//! from pending-upload deletion and preserve its retry source on rejected admission.
use crate::{
    assets,
    store::{random_id, State},
};
use serde_json::{json, Value};
use std::{collections::HashSet, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};
pub(crate) struct Claims {
    pub attachments: Value,
    paths: Vec<PathBuf>,
}
impl Claims {
    pub(crate) fn keep(&mut self) {
        self.paths.clear();
    }
}
impl Drop for Claims {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}
pub(crate) fn claim(s: &State, tid: &str, values: &Value) -> Result<Claims, String> {
    let values = values.as_array().ok_or("Attachments must be an array")?;
    if values.len() > 8 {
        return Err("You can attach up to 8 files per launch".into());
    }
    let total_images = values
        .iter()
        .filter(|v| {
            v["type"] == "image"
                || matches!(
                    v["mimeType"]
                        .as_str()
                        .map(str::to_ascii_lowercase)
                        .as_deref(),
                    Some("image/png" | "image/jpeg" | "image/webp" | "image/gif")
                )
        })
        .fold(0u64, |n, v| {
            n.saturating_add(v["sizeBytes"].as_u64().unwrap_or(0))
        });
    if total_images > 80 * 1024 * 1024 {
        return Err("Images can total up to 80 MiB per launch".into());
    }
    let mut ids = HashSet::new();
    let mut result = Claims {
        attachments: json!([]),
        paths: vec![],
    };
    let segment = tid
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let segment = segment
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .trim_matches(['-', '_'])
        .chars()
        .take(80)
        .collect::<String>()
        .trim_end_matches(['-', '_'])
        .to_owned();
    if segment.is_empty() {
        return Err("Invalid attachment thread identity".into());
    }
    for value in values {
        let id = value["id"].as_str().ok_or("Attachment id required")?;
        if !ids.insert(id) {
            return Err("Duplicate attachment ids are not allowed".into());
        }
        let (_, extension) = assets::parse_id(id)
            .filter(|(p, _)| p == "pending")
            .ok_or("A new thread accepts only pending attachment uploads")?;
        let source = assets::attachment_path(s, id).ok_or("Attachment not found")?;
        let size = std::fs::metadata(&source).map_err(|e| e.to_string())?.len();
        if value["sizeBytes"].as_u64() != Some(size) {
            return Err("Stored attachment size does not match".into());
        }
        let mime = value["mimeType"]
            .as_str()
            .ok_or("Attachment MIME type required")?
            .to_ascii_lowercase();
        let actual = source
            .extension()
            .and_then(|s| s.to_str())
            .ok_or("Invalid attachment path")?;
        if (value["type"] == "image"
            && (extension.is_some()
                || !matches!(
                    (mime.as_str(), actual),
                    ("image/png", "png")
                        | ("image/jpeg", "jpg" | "jpeg")
                        | ("image/gif", "gif")
                        | ("image/webp", "webp")
                )))
            || (value["type"] == "file" && extension.is_none())
        {
            return Err("Attachment type does not match the upload".into());
        }
        let uuid = random_id()?;
        let final_id = format!(
            "{segment}-{uuid}{}",
            extension
                .as_ref()
                .map(|e| format!("-{e}"))
                .unwrap_or_default()
        );
        let destination = assets::attachments_dir(s).join(format!("{final_id}.{actual}"));
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&destination)
            .map_err(|e| e.to_string())?;
        result.paths.push(destination.clone());
        let mut input = std::fs::File::open(&source).map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut input, &mut out).map_err(|e| e.to_string())?;
        if copied != size {
            return Err("Attachment changed while being claimed".into());
        }
        out.flush()
            .and_then(|_| out.sync_all())
            .map_err(|e| e.to_string())?;
        let sidecar = source.with_file_name(format!(
            "{}.txt",
            source.file_name().unwrap().to_string_lossy()
        ));
        if std::fs::symlink_metadata(&sidecar).is_ok_and(|m| m.is_file()) {
            let target = destination.with_file_name(format!(
                "{}.txt",
                destination.file_name().unwrap().to_string_lossy()
            ));
            result.paths.push(target.clone());
            let _ = std::fs::copy(sidecar, target);
        }
        let mut claimed = value.clone();
        claimed.as_object_mut().unwrap().retain(|key, _| {
            matches!(
                key.as_str(),
                "type" | "id" | "name" | "mimeType" | "sizeBytes"
            ) || (key == "source" && value["type"] == "file")
        });
        claimed["id"] = json!(final_id);
        claimed["mimeType"] = json!(mime);
        if value["type"] == "file"
            && mime == "image/svg+xml"
            && !value["name"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase()
                .ends_with(".svg")
        {
            claimed["name"] = json!(format!("{}.svg", value["name"].as_str().unwrap_or("image")));
        }
        result.attachments.as_array_mut().unwrap().push(claimed);
    }
    Ok(result)
}
