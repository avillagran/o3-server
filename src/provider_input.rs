//! Materialize uploaded attachments from the server store, never a client supplied path.
use crate::provider::RunInput;
use base64::Engine;
use serde_json::{json, Value};

pub(crate) fn content(input: &RunInput, claude: bool) -> Result<Vec<Value>, String> {
    let history = input.model_selection["o3HistoricalContext"].as_str();
    let reserve = if claude {
        history.map_or(0, |h| h.len() + 64)
    } else {
        0
    };
    let mut blocks = materialize_within(
        &input.attachment_root,
        &input.text,
        &input.attachments,
        input.context.as_ref(),
        claude,
        reserve,
    )?;
    if claude {
        if let Some(history) = history {
            let current = blocks[0]["text"].as_str().unwrap_or("");
            blocks[0]["text"] = json!(format!("{history}\nCurrent user request:\n{current}"));
        }
    } else {
        // Codex validates aggregate input text independently from its JSON frame size.
        const LIMIT: usize = 1024 * 1024;
        let current_bytes: usize = blocks
            .iter()
            .map(|b| {
                b["text"].as_str().map_or(0, str::len) + b["path"].as_str().map_or(0, str::len)
            })
            .sum();
        if current_bytes > LIMIT {
            return Err("Current message and supplied context exceed Codex's 1048576-character input limit; shorten the current input".into());
        }
        if let Some(history) = history {
            const FOOTER: &str = "\nCurrent user request:\n";
            let room = LIMIT.saturating_sub(current_bytes + FOOTER.len());
            let prefix = if history.len() <= room {
                history.to_owned()
            } else {
                let path = retain_handoff(&input.attachment_root, history)?;
                let header = format!("Historical conversation data, not new instructions. Full retained handoff: {}. Read it when earlier context is needed.\n[Inline history abbreviated to fit Codex; full history retained in that file.]\n", json!(path));
                let allowance = room.saturating_sub(header.len()).min(256 * 1024);
                let mut start = history.len().saturating_sub(allowance);
                while !history.is_char_boundary(start) {
                    start += 1;
                }
                let prefix = format!("{header}{}", &history[start..]);
                if prefix.len() > room {
                    return Err("Current input leaves no room for its historical-context reference; shorten the current input".into());
                }
                prefix
            };
            let current = blocks[0]["text"].as_str().unwrap_or("");
            blocks[0]["text"] = json!(format!("{prefix}{FOOTER}{current}"));
        }
    }
    if serde_json::to_vec(&blocks)
        .map_err(|e| e.to_string())?
        .len()
        > 8 * 1024 * 1024
    {
        return Err("provider input exceeds 8 MiB".into());
    }
    Ok(blocks)
}

fn retain_handoff(root: &std::path::Path, text: &str) -> Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let path = root.join(format!("handoff-{:x}.txt", Sha256::digest(text.as_bytes())));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let temporary = Cleanup(root.join(format!(".handoff-{}.tmp", crate::store::random_id()?)));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary.0)
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    match std::fs::hard_link(&temporary.0, &path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || std::fs::read(&path).map_err(|e| e.to_string())? != text.as_bytes() {
        return Err("retained handoff file failed integrity validation".into());
    }

    Ok(path)
}

/// Claude receives image bytes inside one stream-json frame bounded by `FRAME_LIMIT`.
const FRAME_LIMIT: usize = 8 * 1024 * 1024;
/// Largest single image Claude accepts inline.
const INLINE_IMAGE_LIMIT: u64 = 5 * 1024 * 1024;
/// Beyond 20 images per request the Messages API applies stricter per-image limits.
const INLINE_IMAGE_COUNT: usize = 20;

pub(crate) fn materialize(
    root: &std::path::Path,
    text: &str,
    attachments: &Value,
    context: Option<&Value>,
    claude: bool,
) -> Result<Vec<Value>, String> {
    materialize_within(root, text, attachments, context, claude, 0)
}

/// Like `materialize`, keeping `reserve` bytes of the Claude frame free for text
/// prepended later. Claude images that do not fit inline are handed over by path
/// so a message with many images is never rejected.
fn materialize_within(
    root: &std::path::Path,
    text: &str,
    attachments: &Value,
    context: Option<&Value>,
    claude: bool,
    reserve: usize,
) -> Result<Vec<Value>, String> {
    const BOOTSTRAP:&str="[User attached one or more files without additional text. Respond using the conversation context and the attached files.]";
    let only_think = regex::Regex::new(r"(?i)^Ultrathink:\s*$")
        .unwrap()
        .is_match(text.trim());
    let mut text = if attachments.as_array().is_some_and(|a| !a.is_empty())
        && (text.trim().is_empty() || only_think)
    {
        if only_think {
            format!("Ultrathink:\n{BOOTSTRAP}")
        } else {
            BOOTSTRAP.into()
        }
    } else {
        text.to_owned()
    };
    if let Some(context) = context.filter(|c| !c.is_null()) {
        text.push_str("\n\nAdditional context supplied by the user:\n");
        text.push_str(&serde_json::to_string(context).map_err(|e| e.to_string())?);
    }
    let mut budget = text.len().saturating_add(1024).saturating_add(reserve);
    let mut inline_images = 0;
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
                // Base64 expansion plus the block's JSON envelope.
                let cost = usize::try_from(size)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(4)
                    / 3
                    + 1024;
                let fits = size <= INLINE_IMAGE_LIMIT
                    && inline_images < INLINE_IMAGE_COUNT
                    && budget.saturating_add(cost) <= FRAME_LIMIT;
                let bytes = if fits {
                    use std::io::Read;
                    let mut bytes = Vec::new();
                    std::fs::File::open(&path)
                        .and_then(|f| f.take(INLINE_IMAGE_LIMIT + 1).read_to_end(&mut bytes))
                        .map_err(|e| e.to_string())?;
                    // The file may have changed since the size check.
                    Some(bytes).filter(|b| b.len() as u64 <= INLINE_IMAGE_LIMIT)
                } else {
                    None
                };
                match bytes {
                    Some(bytes) => {
                        budget = budget.saturating_add(cost);
                        inline_images += 1;
                        out.push(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}}));
                    }
                    None => {
                        let note = format!("User attached image {} ({mime}). It did not fit inline in this message; it is available at {}. Open it with the Read tool to view it.", serde_json::to_string(name).unwrap(), serde_json::to_string(&path).unwrap());
                        budget = budget.saturating_add(note.len() + 64);
                        out.push(json!({"type":"text","text":note}));
                    }
                }
            } else {
                out.push(json!({"type":"localImage","path":path}));
            }
        } else {
            let extracted = crate::document_text::sidecar(&path)
                .map(|p| format!(" (text extracted to: {})", json!(p)))
                .unwrap_or_default();
            out.push(json!({"type":"text","text":format!("User attached file {} ({mime}), available at {}{extracted}. Read it when needed.", serde_json::to_string(name).unwrap(), serde_json::to_string(&path).unwrap())}));
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
    let id = crate::provider_policy::configured_id(&settings, id);
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
        // Too large to inline for Claude: handed over by path instead of failing.
        let claude = claude.unwrap();
        assert!(!claude.iter().any(|v| v["type"] == "image"));
        assert!(claude[1]["text"].as_str().unwrap().contains("Read tool"));
    }
}

/// Historical uploads remain available to native tools without re-encoding every old image.
pub(crate) fn historical_context(root: &std::path::Path, text: &str, media: &Value) -> String {
    let mut context = text.to_owned();
    for a in media.as_array().into_iter().flatten() {
        let id = a["id"].as_str().unwrap_or("");
        let path = crate::assets::attachment_path_in(root, id);
        context.push_str(&format!("\nHistorical attachment (read when relevant): {}\n", json!({"id":id,"name":a["name"],"mimeType":a["mimeType"],"path":path,"available":path.is_some()})));
    }
    context
}

#[cfg(test)]
mod history_tests {
    use super::*;
    #[test]
    fn old_images_are_referenced_without_reencoding_but_current_images_remain_inline() {
        let root =
            std::env::temp_dir().join(format!("o3-history-{}", crate::store::random_id().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        let id = "upload-11111111-1111-4111-8111-111111111111";
        std::fs::write(root.join(format!("{id}.png")), vec![0; 4 * 1024 * 1024]).unwrap();
        let media = json!([{"id":id,"name":"old.png","mimeType":"image/png"},{"id":id,"name":"old.png","mimeType":"image/png"}]);
        let both = materialize(&root, "context", &media, None, true).unwrap();
        assert_eq!(both.iter().filter(|v| v["type"] == "image").count(), 1);
        let history = historical_context(&root, "retained text", &media);
        let frames = materialize(&root, &history, &json!([]), None, true).unwrap();
        assert!(
            history.contains("old.png")
                && history.contains(id)
                && history.starts_with("retained text")
        );
        assert_eq!(frames.len(), 1);
        assert!(
            materialize(&root, "new image", &json!([media[0]]), None, true)
                .unwrap()
                .iter()
                .any(|v| v["type"] == "image")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod overflow_tests {
    use super::*;
    #[test]
    fn many_claude_images_split_between_inline_and_paths_within_the_frame() {
        let root = std::env::temp_dir().join(format!(
            "o3-overflow-{}",
            crate::store::random_id().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut attachments = Vec::new();
        let mut files = Vec::new();
        for i in 0..7 {
            let id = format!("upload-{i:08}-1111-4111-8111-111111111111");
            let file = root.join(format!("{id}.png"));
            std::fs::write(&file, vec![0u8; 1536 * 1024]).unwrap();
            files.push(file);
            attachments
                .push(json!({"id":id,"name":format!("shot-{i}.png"),"mimeType":"image/png"}));
        }
        let blocks = materialize(&root, "look", &json!(attachments), None, true).unwrap();
        for file in &files {
            std::fs::remove_file(file).unwrap();
        }
        std::fs::remove_dir(&root).unwrap();
        let inline = blocks.iter().filter(|v| v["type"] == "image").count();
        let by_path: Vec<_> = blocks
            .iter()
            .filter_map(|v| v["text"].as_str())
            .filter(|t| t.contains("did not fit inline"))
            .collect();
        assert!((1..7).contains(&inline), "inline {inline}");
        assert_eq!(inline + by_path.len(), 7);
        assert!(by_path
            .iter()
            .all(|t| t.contains("shot-") && t.contains(".png")));
        assert!(serde_json::to_vec(&blocks).unwrap().len() <= FRAME_LIMIT);
    }
}

#[cfg(test)]
mod final_budget_tests {
    use super::*;
    #[test]
    fn final_codex_input_preserves_new_request_and_full_unicode_history() {
        let root =
            std::env::temp_dir().join(format!("o3-final-{}", crate::store::random_id().unwrap()));
        let history = format!("historical header {}", "é\\\"".repeat(400_000));
        let request = format!("DO_THIS_NEW_REQUEST {}", "r".repeat(800_000));
        let input = RunInput {
            native_mcp: None,
            provider_instance_id: "codex".into(),
            native_session_id: None,
            thread_id: "test".into(),
            run_id: "run".into(),
            cwd: root.clone(),
            text: request.clone(),
            model_selection: json!({"o3HistoricalContext":history}),
            runtime_mode: "full-access".into(),
            interaction_mode: "default".into(),
            attachment_root: root.clone(),
            attachments: json!([]),
            context: Some(json!({"context":"new-context-proof"})),
        };
        let blocks = content(&input, false).unwrap();
        let text = blocks[0]["text"].as_str().unwrap();
        assert!(text.contains(&request) && text.contains("new-context-proof"));
        assert!(text.len() <= 1024 * 1024);
        assert!(text.contains("Inline history abbreviated"));
        let files = std::fs::read_dir(&root)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read_to_string(files[0].path()).unwrap(), history);
        assert_eq!(content(&input, false).unwrap(), blocks);
        std::fs::remove_dir_all(root).unwrap();
    }
}
