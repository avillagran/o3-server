//! Materialize uploaded attachments from the server store, never a client supplied path.
use crate::provider::RunInput;
use base64::Engine;
use serde_json::{json, Value};

/// Version-one records are native context; older arbitrary context values stay compatible.
fn native_context(value: &Value) -> bool {
    value.is_object() && value.get("version").is_some()
}

pub(crate) fn normalize_command_context(
    command: &Value,
) -> Result<std::borrow::Cow<'_, Value>, String> {
    let Some(context) = command.get("context").filter(|value| native_context(value)) else {
        return Ok(std::borrow::Cow::Borrowed(command));
    };
    let context = crate::composer_context::decode_context(context).map_err(str::to_owned)?;
    let mut command = command.clone();
    command["context"] = context;
    Ok(std::borrow::Cow::Owned(command))
}

/// Project native records before provider commands or user-message materialization.
pub(crate) fn project_text(text: &str, context: Option<&Value>) -> Result<String, String> {
    if let Some(context) = context.filter(|value| native_context(value)) {
        let context = crate::composer_context::decode_context(context).map_err(str::to_owned)?;
        return Ok(crate::composer_context::project(
            text,
            context["records"].as_array().unwrap(),
        ));
    }
    Ok(crate::composer_context::project(text, &[]))
}

/// Native context is reference-addressed; arbitrary legacy context remains appended.
pub(crate) fn context_text(text: &str, context: Option<&Value>) -> Result<String, String> {
    append_legacy_context(project_text(text, context)?, context)
}

fn append_legacy_context(mut result: String, context: Option<&Value>) -> Result<String, String> {
    if let Some(context) = context.filter(|value| !value.is_null() && !native_context(value)) {
        result.push_str("\n\nAdditional context supplied by the user:\n");
        result.push_str(&serde_json::to_string(context).map_err(|error| error.to_string())?);
    }
    Ok(result)
}

/// Corrupt legacy native records cannot fabricate entries in a portable handoff.
pub(crate) fn historical_text(text: &str, context: Option<&Value>) -> String {
    project_text(text, context).unwrap_or_else(|_| crate::composer_context::project(text, &[]))
}

pub(crate) fn content(input: &RunInput, claude: bool) -> Result<Vec<Value>, String> {
    let blocks = materialize(
        &input.attachment_root,
        &input.text,
        &input.attachments,
        input.context.as_ref(),
        claude,
    )?;
    finish_content(input, claude, blocks)
}

/// Goal commands already contain one native context envelope; do not project their payload again.
pub(crate) fn projected_content(
    input: &RunInput,
    text: &str,
    claude: bool,
) -> Result<Vec<Value>, String> {
    let text = append_legacy_context(text.to_owned(), input.context.as_ref())?;
    let blocks = materialize_impl(
        &input.attachment_root,
        &text,
        &input.attachments,
        None,
        claude,
        false,
    )?;
    finish_content(input, claude, blocks)
}

fn finish_content(
    input: &RunInput,
    claude: bool,
    mut blocks: Vec<Value>,
) -> Result<Vec<Value>, String> {
    let history = input.model_selection["o3HistoricalContext"].as_str();
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

pub(crate) fn materialize(
    root: &std::path::Path,
    text: &str,
    attachments: &Value,
    context: Option<&Value>,
    claude: bool,
) -> Result<Vec<Value>, String> {
    materialize_impl(root, text, attachments, context, claude, true)
}

fn materialize_impl(
    root: &std::path::Path,
    text: &str,
    attachments: &Value,
    context: Option<&Value>,
    claude: bool,
    project: bool,
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
    if project {
        text = context_text(&text, context)?;
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
        assert!(claude.is_err());
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
        assert!(materialize(&root, "context", &media, None, true).is_err());
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

#[cfg(test)]
mod composer_context_tests {
    use super::*;
    #[test]
    fn both_provider_blocks_and_goal_text_use_the_native_projection_without_raw_context() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/composer-context.json")).unwrap();
        let root = std::path::Path::new("/unused-attachment-root");
        for case in fixture["projections"].as_array().unwrap() {
            let context = json!({"version":1,"records":case["input"]["records"]});
            if crate::composer_context::decode_context(&context).is_err() {
                continue;
            }
            let text = case["input"]["text"].as_str().unwrap();
            for claude in [false, true] {
                let blocks = materialize(root, text, &json!([]), Some(&context), claude).unwrap();
                assert_eq!(blocks[0]["text"], case["expected"], "{}", case["label"]);
            }
            assert_eq!(
                project_text(text, Some(&context)).unwrap(),
                case["expected"].as_str().unwrap()
            );
            assert_eq!(
                historical_text(text, Some(&context)),
                case["expected"].as_str().unwrap()
            );
        }
    }
    #[test]
    fn codex_goal_projection_does_not_recursively_materialize_references_inside_payloads() {
        let input_text = "/goal [selected](t3-context://v1/terminal/term)";
        let literal = "literal [nested](t3-context://v1/skill/not-attached)";
        let context = json!({"version":1,"records":[{"version":1,"kind":"terminal","contextId":"term","label":"Terminal","terminalId":"term","terminalLabel":"Terminal","lineStart":0,"lineEnd":0,"text":literal}]});
        let projected = project_text(input_text, Some(&context)).unwrap();
        let Some(crate::native_goal::Command::Set(objective)) =
            crate::native_goal::parse(&projected, &json!([]))
        else {
            panic!("goal was not parsed");
        };
        let root = std::path::PathBuf::from("/unused-attachment-root");
        let mut input = RunInput {
            native_mcp: None,
            provider_instance_id: "codex".into(),
            native_session_id: None,
            thread_id: "t".into(),
            run_id: "r".into(),
            cwd: root.clone(),
            text: input_text.into(),
            model_selection: json!({}),
            runtime_mode: "full-access".into(),
            interaction_mode: "default".into(),
            attachment_root: root,
            attachments: json!([]),
            context: Some(context),
        };
        let body = format!("Start working toward this goal: {objective}");
        let blocks = projected_content(&input, &body, false).unwrap();
        assert_eq!(blocks[0]["text"], body);
        assert!(body.contains(literal));
        assert_eq!(body.matches("<t3_context version=").count(), 1);
        assert!(!body.contains("unavailable="));
        input.context = Some(json!({"proof":"legacy-goal-context"}));
        let blocks = projected_content(&input, &body, false).unwrap();
        assert_eq!(
            blocks[0]["text"],
            format!(
                "{body}\n\nAdditional context supplied by the user:\n{}",
                input.context.as_ref().unwrap()
            )
        );
    }
    #[test]
    fn missing_records_become_unavailable_and_unversioned_legacy_context_stays_compatible() {
        let text = "[gone](t3-context://v1/terminal/none)";
        assert!(context_text(text, None)
            .unwrap()
            .contains("unavailable=\"true\""));
        let legacy = json!({"records":[{"kind":"terminal","text":"legacy-proof"}]});
        let command = json!({"text":"original","context":legacy});
        assert_eq!(
            normalize_command_context(&command).unwrap().as_ref(),
            &command
        );
        assert_eq!(
            context_text("original", Some(&legacy)).unwrap(),
            format!("original\n\nAdditional context supplied by the user:\n{legacy}")
        );
        assert_eq!(historical_text("original", Some(&legacy)), "original");
        assert!(context_text("original", Some(&json!({"version":2,"records":[]}))).is_err());
    }
}
