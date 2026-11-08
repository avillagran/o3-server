//! Attachments and signed assets, token-compatible with T3: claims are
//! `base64url(json).base64url(hmac-sha256(payload, secrets/asset-access-signing-key.bin))`.
//! File bytes are streamed in both directions; nothing is buffered whole.
use crate::{
    auth,
    body::{self, Body},
    events, paths,
    store::{now_ms, random_id, State},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use hyper::{body::Incoming, header, Request, Response, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicI64, Ordering},
};
use tokio::io::AsyncWriteExt;

type Reply = Result<Value, Value>;
pub const UPLOAD_PREFIX: &str = "/api/attachments/upload/";
pub const ASSET_PREFIX: &str = "/api/assets/";
const UPLOAD_TTL_MS: i64 = 10 * 60_000;
const ASSET_TTL_MS: i64 = 60 * 60_000;
const FAVICON_BUCKET_MS: i64 = 30 * 60_000;
const MAX_IMAGE: u64 = 10 * 1024 * 1024;
const MAX_FILE: u64 = 50 * 1024 * 1024;
const PENDING_MAX_AGE_MS: i64 = 24 * 3_600_000;
const PART_MAX_AGE_MS: i64 = 3_600_000;
const SWEEP_EVERY_MS: i64 = 15 * 60_000;
const MARKER: &str = "project-favicon-missing";
const SAFE_IMAGE_EXT: &[&str] = &[
    "avif", "bmp", "gif", "heic", "heif", "ico", "jpeg", "jpg", "png", "svg", "tiff", "webp",
];

fn invalid(message: impl Into<String>) -> Value {
    json!({"_tag":"RpcInvalidPayload","message":message.into()})
}
fn mac(key: &[u8]) -> Hmac<Sha256> {
    Hmac::new_from_slice(key).expect("hmac accepts any key length")
}
fn signing_key(s: &State) -> std::io::Result<Vec<u8>> {
    auth::load_or_create_key(&s.dir.join("secrets/asset-access-signing-key.bin"))
}
fn sign(s: &State, claims: &Value) -> std::io::Result<String> {
    let payload = B64.encode(claims.to_string());
    let mut m = mac(&signing_key(s)?);
    m.update(payload.as_bytes());
    Ok(format!(
        "{payload}.{}",
        B64.encode(m.finalize().into_bytes())
    ))
}
/// Claims of an unexpired token carrying a valid signature.
fn verify(s: &State, token: &str, kind_ok: impl Fn(&str) -> bool) -> Option<Value> {
    let (payload, sig) = auth::token_parts(token)?;
    let mut m = mac(&signing_key(s).ok()?);
    m.update(payload.as_bytes());
    m.verify_slice(&auth::decode_b64url(sig)?).ok()?;
    let claims: Value = serde_json::from_slice(&auth::decode_b64url(payload)?).ok()?;
    if let Some(sid) = claims["sessionId"].as_str() {
        let valid: bool = s.db.lock().ok()?.query_row("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE session_id=? AND revoked_at IS NULL AND expires_at>?)", rusqlite::params![sid,now_ms()], |r| r.get(0)).ok()?;
        if !valid {
            return None;
        }
    }
    (claims["version"] == 1
        && kind_ok(claims["kind"].as_str()?)
        && claims["expiresAt"].as_i64()? > now_ms())
    .then_some(claims)
}

pub fn attachments_dir(s: &State) -> PathBuf {
    s.dir.join("attachments")
}

// ------------------------------------------------------------------ attachment ids

/// `<thread-segment>-<uuid>[-<ext>]`, as T3 names attachments. Returns (segment, ext).
pub(crate) fn parse_id(id: &str) -> Option<(String, Option<String>)> {
    if id.is_empty() || id.len() > 256 || id.contains(['/', '\\', '.', '\0']) {
        return None;
    }
    let id = id.to_lowercase();
    let with_uuid = |s: &str| -> Option<String> {
        let (seg, uuid) = s.len().checked_sub(37).and_then(|i| {
            (s.is_char_boundary(i) && s.as_bytes()[i] == b'-').then(|| (&s[..i], &s[i + 1..]))
        })?;
        let groups: Vec<&str> = uuid.split('-').collect();
        let shape = [8, 4, 4, 4, 12];
        let uuid_ok = groups.len() == 5
            && groups
                .iter()
                .zip(shape)
                .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()));
        let seg_ok = !seg.is_empty()
            && seg.split('-').all(|p| {
                !p.is_empty()
                    && p.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            });
        (uuid_ok && seg_ok).then(|| seg.to_owned())
    };
    if let Some(seg) = with_uuid(&id) {
        return Some((seg, None));
    }
    let (rest, ext) = id.rsplit_once('-')?;
    if ext.is_empty()
        || ext.len() > 10
        || !ext
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return None;
    }
    with_uuid(rest).map(|seg| (seg, Some(ext.to_owned())))
}

/// The stored file of attachment `id`, if the id is well formed and the file exists.
pub fn attachment_path(s: &State, id: &str) -> Option<PathBuf> {
    attachment_path_in(&attachments_dir(s), id)
}

pub(crate) fn attachment_path_in(dir: &Path, id: &str) -> Option<PathBuf> {
    let (_, ext) = parse_id(id)?;
    let id = id.to_lowercase();
    match ext {
        Some(ext) => Some(dir.join(format!("{id}.{ext}")))
            .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file())),
        None => SAFE_IMAGE_EXT
            .iter()
            .chain(&["bin"])
            .map(|e| dir.join(format!("{id}.{e}")))
            .find(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file())),
    }
}

pub(crate) fn cleanup_attachment(s: &State, id: &str) -> Result<(), String> {
    let Some((_, extension)) = parse_id(id) else {
        return Ok(());
    };
    let extensions = extension.map(|e| vec![e]).unwrap_or_else(|| {
        SAFE_IMAGE_EXT
            .iter()
            .chain(&["bin"])
            .map(|e| e.to_string())
            .collect()
    });
    for extension in extensions {
        let path = attachments_dir(s).join(format!("{}.{}", id.to_lowercase(), extension));
        let mut sidecar = path.clone().into_os_string();
        sidecar.push(".txt");
        for path in [path, std::path::PathBuf::from(sidecar)] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    Ok(())
}

fn file_extension(name: &str) -> String {
    match name.rsplit_once('.').map(|(_, e)| e.to_lowercase()) {
        Some(e)
            if e != "part"
                && !e.is_empty()
                && e.len() <= 10
                && e.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) =>
        {
            e
        }
        _ => "bin".into(),
    }
}
fn image_extension(mime: &str, name: &str) -> String {
    match mime.to_lowercase().as_str() {
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        "image/gif" => "gif",
        "image/heic" => "heic",
        "image/heif" => "heif",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/svg+xml" => "svg",
        "image/tiff" => "tiff",
        "image/webp" => "webp",
        _ => {
            let e = file_extension(name);
            return if SAFE_IMAGE_EXT.contains(&e.as_str()) {
                e
            } else {
                "bin".into()
            };
        }
    }
    .into()
}

// ------------------------------------------------------------------ attachments.createUploadUrl / delete

static LAST_SWEEP: AtomicI64 = AtomicI64::new(0);

/// Drops pending uploads older than a day and abandoned `.part` files older than an hour.
fn sweep(dir: &Path) {
    let now = std::time::SystemTime::now();
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let age = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|m| now.duration_since(m).ok())
            .map(|d| d.as_millis() as i64);
        let stale = if name.ends_with(".part") {
            age.is_some_and(|a| a > PART_MAX_AGE_MS)
        } else {
            name.starts_with("pending-") && age.is_some_and(|a| a > PENDING_MAX_AGE_MS)
        };
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

pub fn create_upload_url(s: &State, p: &Value) -> Reply {
    let kind = match p["type"].as_str() {
        None | Some("image") => "image",
        Some("file") => "file",
        Some(_) => return Err(invalid("type must be image or file")),
    };
    let name = p["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty() && n.chars().count() <= 255)
        .ok_or_else(|| invalid("name must be 1-255 characters"))?;
    let mime = p["mimeType"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty() && m.chars().count() <= 100)
        .ok_or_else(|| invalid("mimeType must be 1-100 characters"))?;
    let size = p["sizeBytes"]
        .as_u64()
        .filter(|n| *n >= 1)
        .ok_or_else(|| invalid("sizeBytes must be a positive integer"))?;
    if kind == "image"
        && !matches!(
            mime,
            "image/gif" | "image/jpeg" | "image/png" | "image/webp"
        )
    {
        return Err(invalid("unsupported image mimeType"));
    }
    if size > if kind == "image" { MAX_IMAGE } else { MAX_FILE } {
        return Err(invalid("sizeBytes exceeds the attachment limit"));
    }
    let now = now_ms();
    let last = LAST_SWEEP.load(Ordering::Relaxed);
    if now - last >= SWEEP_EVERY_MS
        && LAST_SWEEP
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        let dir = attachments_dir(s);
        std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || sweep(&dir))
            .ok();
    }
    let name = if kind == "file"
        && mime.eq_ignore_ascii_case("image/svg+xml")
        && !name.to_lowercase().ends_with(".svg")
    {
        format!("{name}.svg")
    } else {
        name.to_owned()
    };
    let uuid = random_id().map_err(invalid)?;
    let id = if kind == "file" {
        format!("pending-{uuid}-{}", file_extension(&name))
    } else {
        format!("pending-{uuid}")
    };
    let expires = now + UPLOAD_TTL_MS;
    let claims = json!({"version":1,"kind":"attachment-upload","sessionId":p["_assetSession"],"type":kind,"attachmentId":id,"name":name,"mimeType":mime,"sizeBytes":size,"expiresAt":expires});
    let token = sign(s, &claims)
        .map_err(|e| json!({"_tag":"AttachmentUploadSigningKeyError","cause":e.to_string()}))?;
    Ok(
        json!({"attachmentId":id,"relativeUrl":format!("{UPLOAD_PREFIX}{token}"),"expiresAt":expires}),
    )
}

pub fn delete_attachment(s: &State, p: &Value) -> Reply {
    let id = p["attachmentId"]
        .as_str()
        .map(str::trim)
        .filter(|i| !i.is_empty() && i.len() <= 256)
        .ok_or_else(|| invalid("attachmentId is required"))?;
    // Only pending uploads are deletable here; thread attachments belong to their threads.
    if parse_id(id).is_some_and(|(seg, _)| seg == "pending") {
        if let Some(path) = attachment_path(s, id) {
            let _ = std::fs::remove_file(&path);
            let mut txt = path.into_os_string();
            txt.push(".txt");
            let _ = std::fs::remove_file(txt);
        }
    }
    Ok(Value::Null)
}

fn text(status: StatusCode, message: &str) -> Response<Body> {
    let mut r = Response::new(body::full(message.to_owned()));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}
fn status_only(status: StatusCode) -> Response<Body> {
    let mut r = Response::new(body::empty());
    *r.status_mut() = status;
    r
}

fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(h.finalize().into());
        }
        h.update(&buf[..n]);
    }
}

/// `POST /api/attachments/upload/<token>`: raw bytes, exactly the declared size, 204 on success.
pub async fn upload(s: &State, token: &str, request: Request<Incoming>) -> Response<Body> {
    let Some(claims) = verify(s, token, |k| k == "attachment-upload") else {
        return text(StatusCode::NOT_FOUND, "Not Found");
    };
    let size = claims["sizeBytes"].as_u64().unwrap_or(0);
    if let Some(len) = request.headers().get(header::CONTENT_LENGTH) {
        if len.to_str().ok().and_then(|v| v.parse::<u64>().ok()) != Some(size) {
            return text(
                StatusCode::BAD_REQUEST,
                "Content-Length must match the upload size.",
            );
        }
    }
    let (Some(id), Some(name), Some(mime)) = (
        claims["attachmentId"].as_str(),
        claims["name"].as_str(),
        claims["mimeType"].as_str(),
    ) else {
        return text(StatusCode::NOT_FOUND, "Not Found");
    };
    let ext = if claims["type"] == "file" {
        file_extension(name)
    } else {
        image_extension(mime, name)
    };
    let dir = attachments_dir(s);
    let (final_path, part) = (
        dir.join(format!("{id}.{ext}")),
        dir.join(format!(
            "{id}.{ext}.{}.part",
            random_id().unwrap_or_default()
        )),
    );
    // Ids come from our own signature, but never write outside the attachments folder anyway.
    if parse_id(id).is_none() || final_path.parent() != Some(dir.as_path()) {
        return text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to resolve attachment path.",
        );
    }
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to persist upload.",
        );
    }
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        store(request.into_body(), &part, &final_path, size),
    )
    .await
    .unwrap_or_else(|_| Err((StatusCode::REQUEST_TIMEOUT, "Upload timed out".into())));
    let _ = tokio::fs::remove_file(&part).await;
    match result {
        Ok(()) => status_only(StatusCode::NO_CONTENT),
        Err((status, detail)) => text(status, &detail),
    }
}

async fn store(
    mut incoming: Incoming,
    part: &Path,
    final_path: &Path,
    size: u64,
) -> Result<(), (StatusCode, String)> {
    let failed = |_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to persist upload.".to_owned(),
        )
    };
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(part)
        .await
        .map_err(failed)?;
    let mut received = 0u64;
    while let Some(frame) = incoming.frame().await {
        let Ok(frame) = frame else {
            return Err((StatusCode::BAD_REQUEST, "Upload was interrupted.".into()));
        };
        if let Some(data) = frame.data_ref() {
            received += data.len() as u64;
            if received > size {
                break;
            }
            file.write_all(data).await.map_err(failed)?;
        }
    }
    file.flush().await.map_err(failed)?;
    drop(file);
    if received != size {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Body was {received} bytes, expected {size}."),
        ));
    }
    // Publishing never replaces an existing upload: an identical copy is fine, a different one conflicts.
    match tokio::fs::hard_link(part, final_path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let (a, b) = (part.to_owned(), final_path.to_owned());
            let same = tokio::task::spawn_blocking(
                move || matches!((sha256_file(&a), sha256_file(&b)), (Ok(x), Ok(y)) if x == y),
            )
            .await
            .unwrap_or(false);
            if same {
                Ok(())
            } else {
                Err((StatusCode::CONFLICT, "Attachment already uploaded.".into()))
            }
        }
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to persist upload.".into(),
        )),
    }
}

// ------------------------------------------------------------------ assets.createUrl

pub fn mime_for(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "aac" => "audio/aac",
        "avi" => "video/x-msvideo",
        "ogv" => "video/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "icns" => "image/icns",
        _ => return None,
    })
}
/// Types a workspace file may be previewed as (the server never signs arbitrary file types).
fn previewable(path: &str) -> bool {
    mime_for(path).is_some_and(|m| {
        m != "image/icns" && !m.starts_with("font/") && m != "text/css" && m != "text/javascript"
    })
}
const DEPENDENCY_EXT: &[&str] = &[
    "css", "js", "mjs", "otf", "ttf", "woff", "woff2", "avif", "gif", "ico", "jpeg", "jpg", "png",
    "svg", "webp",
];

fn asset_err(tag: &str, resource: &Value) -> Value {
    json!({"_tag":tag,"resource":resource})
}

fn image_dimensions(path: &Path) -> Option<Value> {
    let mut head = Vec::new();
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o4000)
        .open(path)
        .ok()?
        .take(64 * 1024)
        .read_to_end(&mut head)
        .ok()?;
    image_dimensions_bytes(&head)
}
fn image_dimensions_bytes(b: &[u8]) -> Option<Value> {
    let be32 = |i: usize| {
        b.get(i..i + 4)
            .map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
    };
    let le16 = |i: usize| {
        b.get(i..i + 2)
            .map(|x| u16::from_le_bytes([x[0], x[1]]) as u32)
    };
    let (w, h) = if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        (be32(16)?, be32(20)?)
    } else if b.starts_with(b"GIF8") {
        (le16(6)?, le16(8)?)
    } else if b.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        loop {
            if *b.get(i)? != 0xff {
                i += 1;
                continue;
            }
            let m = *b.get(i + 1)?;
            if (0xc0..=0xcf).contains(&m) && !matches!(m, 0xc4 | 0xc8 | 0xcc) {
                let hh = u16::from_be_bytes([*b.get(i + 5)?, *b.get(i + 6)?]) as u32;
                let ww = u16::from_be_bytes([*b.get(i + 7)?, *b.get(i + 8)?]) as u32;
                break (ww, hh);
            }
            if m == 0xd8 || m == 0x01 || (0xd0..=0xd7).contains(&m) {
                i += 2;
                continue;
            }
            i += 2 + u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]) as usize;
        }
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        match b.get(12..16)? {
            b"VP8X" => (
                1 + (le16(24)? | (*b.get(26)? as u32) << 16),
                1 + (le16(27)? | (*b.get(29)? as u32) << 16),
            ),
            b"VP8L" => {
                let v = u32::from_le_bytes([*b.get(21)?, *b.get(22)?, *b.get(23)?, *b.get(24)?]);
                (1 + (v & 0x3fff), 1 + ((v >> 14) & 0x3fff))
            }
            b"VP8 " => (le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            _ => return None,
        }
    } else {
        return None;
    };
    (w > 0 && h > 0).then(|| json!({"width":w,"height":h}))
}

/// The directory a thread's relative paths resolve against: its worktree, else its project's root.
fn thread_root(s: &State, thread_id: &str) -> Option<PathBuf> {
    let db = s.db.lock().unwrap();
    let thread = events::get(&db, "thread", thread_id).ok()?;
    if let Some(w) = thread["worktreePath"].as_str().filter(|w| !w.is_empty()) {
        return Some(PathBuf::from(w));
    }
    let project = events::get(&db, "project", thread["projectId"].as_str()?).ok()?;
    project["workspaceRoot"].as_str().map(PathBuf::from)
}

pub fn required_scope(p: &Value) -> &'static str {
    if p["resource"]["_tag"] == "tool-output-image" {
        "orchestration:read"
    } else {
        "filesystem:read"
    }
}

fn protected_path(s: &State, real: &Path) -> bool {
    std::fs::canonicalize(&s.asset_profile_root).is_ok_and(|r| real.starts_with(r))
        || std::env::var_os("HOME")
            .and_then(|h| std::fs::canonicalize(h).ok())
            .is_some_and(|home| {
                real.strip_prefix(home).ok().is_some_and(|rel| {
                    !rel.starts_with(".local/share/icons")
                        && !rel.starts_with(".local/share/pixmaps")
                        && rel
                            .components()
                            .next()
                            .is_none_or(|c| c.as_os_str().to_string_lossy().starts_with('.'))
                })
            })
        || real.components().any(|c| {
            let name = c.as_os_str().to_string_lossy();
            matches!(
                name.as_ref(),
                ".ssh"
                    | ".aws"
                    | ".codex"
                    | ".claude"
                    | ".gnupg"
                    | ".config"
                    | ".git"
                    | ".netrc"
                    | ".npmrc"
                    | "secrets"
            ) || name == ".env"
                || name.starts_with(".env.")
        })
}
// Recompute canonical boundaries on every request, including signed downloads.
fn allowed_file(s: &State, path: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(path).ok()?;
    let profile = std::fs::canonicalize(&s.dir).ok()?;
    let attachments = std::fs::canonicalize(attachments_dir(s))
        .ok()
        .filter(|r| *r == profile.join("attachments"));
    let cache = std::fs::canonicalize(s.dir.join("asset-media"))
        .ok()
        .filter(|r| *r == profile.join("asset-media"));
    if attachments
        .iter()
        .chain(cache.iter())
        .any(|r| real.starts_with(r) && real != *r)
    {
        return Some(real);
    }
    if protected_path(s, &real) {
        return None;
    }
    let db = s.db.lock().ok()?;
    let mut q = db
        .prepare("SELECT body FROM records WHERE kind IN ('project','thread')")
        .ok()?;
    let rows = q.query_map([], |r| r.get::<_, String>(0)).ok()?;
    for row in rows.flatten() {
        let v: Value = serde_json::from_str(&row).ok()?;
        for key in ["workspaceRoot", "worktreePath"] {
            if let Some(root) = v[key].as_str().and_then(|r| std::fs::canonicalize(r).ok()) {
                if real.starts_with(&root) && real != root {
                    return Some(real);
                }
            }
        }
    }
    None
}

/// Host media (`media-file`, absolute draft files): as in T3, anything the chat can reference,
/// wherever it is, for a `filesystem:read` session — viewing a previewable type or downloading
/// any file. Only this server's own profile (database, session signing key) stays refused.
fn media_allowed(s: &State, real: &Path) -> bool {
    allowed_file(s, real).is_some()
        || !std::fs::canonicalize(&s.asset_profile_root).is_ok_and(|r| real.starts_with(r))
}

fn real_dir(root: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(root).ok().filter(|r| r.is_dir())
}

/// Canonical regular file at `relative` under the real `root`, proven to stay inside it.
fn workspace_file(root: &Path, relative: &str) -> Option<(PathBuf, String)> {
    let real_root = real_dir(root)?;
    let (abs, rel) = paths::resolve_relative(&real_root, relative)?;
    let real = std::fs::canonicalize(&abs).ok()?;
    (paths::within(&real_root, &real) && real != real_root && real.is_file()).then_some((real, rel))
}

fn media_revision(m: &std::fs::Metadata) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        m.dev(),
        m.ino(),
        m.size(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec()
    )
}

fn media_claims(real: &Path, download: bool, expires: i64) -> Option<(Value, String, String)> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o4000)
        .open(real)
        .ok()?;
    let m = f.metadata().ok().filter(|m| m.is_file())?;
    let name = real.file_name()?.to_string_lossy().into_owned();
    let revision = media_revision(&m);
    let mut c = json!({"version":1,"kind":"media-file-exact","filePath":real.to_string_lossy(),"device":m.dev().to_string(),"inode":m.ino().to_string(),"fileRevision":revision,"expiresAt":expires});
    if download {
        c["download"] = json!(true);
    }
    Some((c, name, revision))
}

fn favicon_candidates() -> &'static [&'static str] {
    &[
        "favicon.ico",
        "favicon.svg",
        "favicon.png",
        "public/favicon.ico",
        "public/favicon.svg",
        "public/favicon.png",
        "app/favicon.ico",
        "app/icon.svg",
        "app/icon.png",
        "src/app/favicon.ico",
        "src/app/icon.svg",
        "src/app/icon.png",
        "static/favicon.ico",
        "static/favicon.svg",
        "static/favicon.png",
        "assets/icon.png",
        "assets/icon.svg",
        "assets/logo.png",
        "assets/logo.svg",
    ]
}
fn favicon_ext_ok(p: &str) -> bool {
    p.len() <= 1024
        && matches!(
            p.rsplit_once('.').map(|(_, e)| e.to_lowercase()).as_deref(),
            Some("avif" | "gif" | "icns" | "ico" | "jpg" | "jpeg" | "png" | "svg" | "webp")
        )
}

/// Follow only index.theme-declared paths, with finite inheritance and directory budgets.
fn bounded_text(path: impl AsRef<Path>) -> std::io::Result<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o4000)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut text = String::new();
    file.take(65537).read_to_string(&mut text)?;
    if text.len() > 65536 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(text)
}
fn icon_allowed(real: &Path) -> bool {
    let mut roots = vec![
        PathBuf::from("/usr/share"),
        PathBuf::from("/usr/local/share"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".local/share"));
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        roots.push(data.into());
    }
    if let Some(data) = std::env::var_os("XDG_DATA_DIRS") {
        roots.extend(std::env::split_paths(&data).take(32));
    }
    roots
        .iter()
        .flat_map(|r| [r.join("icons"), r.join("pixmaps")])
        .any(|r| std::fs::canonicalize(r).is_ok_and(|r| real.starts_with(&r) && real != r))
}
fn themed_icon(roots: &[PathBuf], icon: &str) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    let theme = config
        .and_then(|p| bounded_text(p.join("gtk-3.0/settings.ini")).ok())
        .and_then(|text| {
            text.lines().find_map(|l| {
                l.trim()
                    .strip_prefix("gtk-icon-theme-name=")
                    .map(|s| s.trim().to_owned())
            })
        });
    let mut pending: std::collections::VecDeque<String> =
        theme.into_iter().chain(["hicolor".into()]).collect();
    let mut visited = std::collections::HashSet::new();
    while let Some(theme) = pending.pop_front() {
        if visited.len() >= 16 {
            break;
        }
        if !safe_theme_path(&theme) || theme.contains('/') || !visited.insert(theme.clone()) {
            continue;
        }
        for root in roots {
            let base = root.join("icons").join(&theme);
            let index = bounded_text(base.join("index.theme")).unwrap_or_default();
            let mut dirs: Vec<String> = Vec::new();
            for line in index.lines() {
                if let Some(inherits) = line.strip_prefix("Inherits=") {
                    for inherited in inherits
                        .split(',')
                        .take(16)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        pending.push_front(inherited.trim().to_owned());
                    }
                }
                if let Some(list) = line
                    .strip_prefix("Directories=")
                    .or_else(|| line.strip_prefix("ScaledDirectories="))
                {
                    dirs.extend(list.split(',').take(256).map(|s| s.trim().to_owned()));
                }
            }
            if theme == "hicolor" {
                dirs.extend(
                    [
                        "scalable", "256x256", "128x128", "64x64", "48x48", "32x32", "16x16",
                    ]
                    .map(|s| format!("{s}/apps")),
                );
            }
            for dir in dirs.into_iter().take(256).filter(|d| safe_theme_path(d)) {
                for ext in ["png", "svg"] {
                    let p = base.join(&dir).join(format!("{icon}.{ext}"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    for root in roots {
        for ext in ["png", "svg"] {
            let p = root.join("pixmaps").join(format!("{icon}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}
fn safe_theme_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        && !path.contains('\\')
}
fn cached_media_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Some("wav")
    } else if bytes.get(4..8) == Some(b"ftyp") {
        if bytes.get(8..12) == Some(b"avif") {
            Some("avif")
        } else if bytes.get(8..12) == Some(b"M4A ") {
            Some("m4a")
        } else {
            Some("mp4")
        }
    } else if bytes.starts_with(b"ID3") || bytes.starts_with(b"\xff\xfb") {
        Some("mp3")
    } else if bytes.starts_with(b"fLaC") {
        Some("flac")
    } else {
        None
    }
}
fn media_cache_has_room(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| entries.take(128).count() < 128)
}
fn sweep_media(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.take(2048).flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() || mime_for(&entry.path().to_string_lossy()).is_none() {
            continue;
        }
        // Two grant lifetimes leave an in-flight URL valid through its entire TTL.
        if meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age.as_millis() > (2 * ASSET_TTL_MS) as u128)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Credentialed media is fetched once into the private profile, then served through an expiring grant.
pub async fn create_url_async(s: &std::sync::Arc<State>, p: &Value) -> Reply {
    let resource = &p["resource"];
    if resource["_tag"] != "github-media" {
        let (s, p) = (s.clone(), p.clone());
        return tokio::task::spawn_blocking(move || create_url(&s, &p))
            .await
            .unwrap_or_else(|_| Err(invalid("asset lookup failed")));
    }
    let raw = resource["url"]
        .as_str()
        .ok_or_else(|| invalid("url required"))?;
    let url = url::Url::parse(raw).map_err(|_| invalid("invalid media URL"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "private-user-images.githubusercontent.com"
                    | "user-images.githubusercontent.com"
            )
        )
    {
        return Err(invalid("unsupported media host"));
    }
    let cwd = resource["cwd"]
        .as_str()
        .ok_or_else(|| invalid("cwd required"))?;
    let cwd = std::fs::canonicalize(cwd).map_err(|_| invalid("invalid cwd"))?;
    let known = {
        let db = s.reader.lock().unwrap();
        let mut q = db
            .prepare("SELECT body FROM records WHERE kind IN ('project','thread')")
            .map_err(|_| invalid("workspace lookup failed"))?;
        let rows = q
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|_| invalid("workspace lookup failed"))?;
        let known = rows.flatten().any(|body| {
            serde_json::from_str::<Value>(&body).ok().is_some_and(|v| {
                ["workspaceRoot", "worktreePath"].iter().any(|key| {
                    v[key]
                        .as_str()
                        .and_then(|r| std::fs::canonicalize(r).ok())
                        .is_some_and(|r| cwd == r)
                })
            })
        });
        known
    };
    if !known {
        return Err(invalid("unregistered workspace"));
    }
    let o = crate::proc::run(
        "gh",
        &cwd,
        &["api", raw],
        crate::proc::Opts {
            timeout: std::time::Duration::from_secs(30),
            cap: 10 * 1024 * 1024,
            allow_truncate: false,
        },
        &[],
    )
    .await
    .map_err(|_| asset_err("AssetWorkspaceResolutionError", resource))?;
    if !o.ok() {
        return Err(asset_err("AssetWorkspaceAssetNotFoundError", resource));
    }
    let dir = s.dir.join("asset-media");
    std::fs::create_dir_all(&dir).map_err(|_| invalid("asset cache unavailable"))?;
    static CACHE_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = CACHE_WRITE.lock().unwrap();
    sweep_media(&dir);
    if !media_cache_has_room(&dir) {
        return Err(invalid("asset cache full"));
    }
    let ext =
        cached_media_extension(&o.stdout).ok_or_else(|| invalid("unsupported media content"))?;
    let file = dir.join(format!(
        "{}.{ext}",
        random_id().map_err(|_| invalid("asset id unavailable"))?
    ));
    std::fs::write(&file, &o.stdout).map_err(|_| invalid("asset cache write failed"))?;
    let expires = now_ms() + ASSET_TTL_MS;
    let (mut claims, name, revision) =
        media_claims(&file, false, expires).ok_or_else(|| invalid("asset cache unavailable"))?;
    claims["sessionId"] = p["_assetSession"].clone();
    claims["requiredScope"] = json!("filesystem:read");
    let token = sign(s, &claims).map_err(|_| invalid("asset signing failed"))?;
    Ok(
        json!({"relativeUrl":format!("{ASSET_PREFIX}{token}/{name}"),"expiresAt":expires,"fileRevision":revision}),
    )
}

pub fn create_url(s: &State, p: &Value) -> Reply {
    let resource = &p["resource"];
    let tag = resource["_tag"]
        .as_str()
        .ok_or_else(|| invalid("resource._tag is required"))?;
    let text = |k: &str, max: usize| {
        resource[k]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty() && v.chars().count() <= max)
            .map(str::to_owned)
    };
    let mut expires = now_ms() + ASSET_TTL_MS;
    let (mut claims, file_name, mut dims, mut revision, mut source): (
        Value,
        String,
        Option<Value>,
        Option<String>,
        Option<String>,
    );
    dims = None;
    revision = None;
    source = None;
    let not_found = |t: &str| asset_err(t, resource);
    match tag {
        "attachment" => {
            let id =
                text("attachmentId", 256).ok_or_else(|| invalid("attachmentId is required"))?;
            let path =
                attachment_path(s, &id).ok_or_else(|| not_found("AssetAttachmentNotFoundError"))?;
            allowed_file(s, &path).ok_or_else(|| not_found("AssetWorkspacePathValidationError"))?;
            let ext = parse_id(&id).and_then(|(_, e)| e);
            let generic = ext.is_some();
            let mime = text("mimeType", 100);
            let video = mime
                .as_deref()
                .map(|m| m.split(';').next().unwrap_or("").trim().to_lowercase())
                .filter(|m| {
                    m.starts_with("video/")
                        && m[6..]
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"!#$&^.+-_".contains(&b))
                });
            let inline_mime = (resource["disposition"] == "inline")
                .then(|| {
                    ext.as_deref().and_then(|e| match e {
                        "svg" => Some("image/svg+xml"),
                        "pdf" => Some("application/pdf"),
                        "html" | "htm" => Some("text/html"),
                        e => mime_for(&format!("x.{e}")),
                    })
                })
                .flatten();
            if !generic {
                dims = image_dimensions(&path);
            }
            claims = json!({"version":1,"kind":"attachment","attachmentId":id});
            if generic && video.is_none() && inline_mime.is_none() {
                claims["download"] = json!(true);
            }
            if let Some(n) = text("fileName", 255) {
                claims["fileName"] = json!(n);
            }
            if let Some(m) = inline_mime {
                claims["mimeType"] = json!(m);
            } else if let Some(m) = video.clone().or(mime) {
                claims["mimeType"] = json!(m);
            }
            if resource["renderIntent"] == "mcp-app" && inline_mime == Some("text/html") {
                claims["renderIntent"] = json!("mcp-app");
            }
            file_name = text("fileName", 255).unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            if claims["mimeType"] == "text/html" {
                expires = (expires + 30 * 60_000 - 1) / (30 * 60_000) * (30 * 60_000);
            }
        }
        "workspace-file" | "draft-workspace-file" | "media-file" => {
            let path = text("path", 1024).ok_or_else(|| invalid("path is required"))?;
            let root = if tag == "draft-workspace-file" {
                Some(PathBuf::from(
                    text("cwd", 1024).ok_or_else(|| invalid("cwd is required"))?,
                ))
            } else {
                let tid = text("threadId", 256).ok_or_else(|| invalid("threadId is required"))?;
                Some(
                    thread_root(s, &tid)
                        .ok_or_else(|| not_found("AssetWorkspaceContextNotFoundError"))?,
                )
            };
            let host = paths::expand_home(&path);
            let download = tag == "media-file" && resource["disposition"] == "attachment";
            if tag == "media-file" || (tag == "draft-workspace-file" && host.is_absolute()) {
                // Host-local media: absolute, or relative to the thread's workspace.
                let target = if host.is_absolute() {
                    host
                } else {
                    root.as_deref()
                        .and_then(real_dir)
                        .map(|r| paths::normalize(&r.join(&host)))
                        .ok_or_else(|| not_found("AssetWorkspaceContextNotFoundError"))?
                };
                let real = std::fs::canonicalize(&target)
                    .ok()
                    .filter(|r| r.is_file())
                    .ok_or_else(|| asset_err("AssetWorkspaceAssetNotFoundError", resource))?;
                if !media_allowed(s, &real) {
                    return Err(not_found("AssetWorkspacePathValidationError"));
                }
                if !download && !previewable(&real.to_string_lossy()) {
                    return Err(not_found("AssetPreviewTypeValidationError"));
                }
                let (c, name, rev) = media_claims(&real, download, expires)
                    .ok_or_else(|| not_found("AssetWorkspaceAssetNotFoundError"))?;
                claims = c;
                file_name = name;
                revision = Some(rev);
                dims = image_dimensions(&real);
            } else {
                let root = root.unwrap();
                let real_root = real_dir(&root)
                    .ok_or_else(|| not_found("AssetWorkspaceRootNormalizationError"))?;
                // An absolute path inside the root is turned into a relative one first.
                let relative = if Path::new(&path).is_absolute() {
                    Path::new(&path)
                        .strip_prefix(&real_root)
                        .ok()
                        .map(|r| r.to_string_lossy().into_owned())
                        .unwrap_or(path.clone())
                } else {
                    path.clone()
                };
                let (_, rel) = paths::resolve_relative(&real_root, &relative)
                    .ok_or_else(|| not_found("AssetWorkspacePathValidationError"))?;
                if !previewable(&rel) {
                    return Err(not_found("AssetPreviewTypeValidationError"));
                }
                let (real, rel) = workspace_file(&real_root, &rel)
                    .ok_or_else(|| not_found("AssetWorkspaceAssetNotFoundError"))?;
                allowed_file(s, &real)
                    .ok_or_else(|| not_found("AssetWorkspacePathValidationError"))?;
                dims = image_dimensions(&real);
                file_name = Path::new(&rel)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                claims = if rel.to_lowercase().ends_with(".html")
                    || rel.to_lowercase().ends_with(".htm")
                {
                    let base = Path::new(&rel)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .filter(|p| !p.is_empty())
                        .unwrap_or(".".into());
                    json!({"version":1,"kind":"workspace-file","workspaceRoot":real_root.to_string_lossy(),"baseRelativePath":base,"expiresAt":expires})
                } else {
                    json!({"version":1,"kind":"workspace-file-exact","workspaceRoot":real_root.to_string_lossy(),"relativePath":rel,"expiresAt":expires})
                };
            }
        }
        "native-app-icon" => {
            let app = &resource["app"];
            let id = app["appId"]
                .as_str()
                .or_else(|| app["displayName"].as_str())
                .filter(|v| {
                    !v.is_empty()
                        && v.len() <= 512
                        && !v.chars().any(char::is_control)
                        && (app["_tag"] == "display-name"
                            || v.bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
                })
                .ok_or_else(|| invalid("invalid application reference"))?;
            let mut roots = Vec::new();
            if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
                roots.push(PathBuf::from(data));
            }
            if let Some(home) = std::env::var_os("HOME") {
                roots.push(PathBuf::from(home).join(".local/share"));
            }
            roots.extend(
                std::env::split_paths(
                    &std::env::var_os("XDG_DATA_DIRS")
                        .unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
                )
                .take(32),
            );
            let mut icon = None;
            for root in &roots {
                if app["_tag"] == "display-name" {
                    if let Ok(entries) = std::fs::read_dir(root.join("applications")) {
                        for entry in entries.take(2048).flatten() {
                            let Ok(text) = bounded_text(entry.path()) else {
                                continue;
                            };
                            if text.lines().any(|line| {
                                line.strip_prefix("Name=")
                                    .is_some_and(|name| name.eq_ignore_ascii_case(id))
                            }) {
                                icon = text
                                    .lines()
                                    .find_map(|line| line.strip_prefix("Icon=").map(str::to_owned));
                                if icon.is_some() {
                                    break;
                                }
                            }
                        }
                    }
                    if icon.is_some() {
                        break;
                    } else {
                        continue;
                    }
                }
                let desktop = root
                    .join("applications")
                    .join(format!("{}.desktop", id.trim_end_matches(".desktop")));
                if let Ok(text) = bounded_text(desktop) {
                    icon = text
                        .lines()
                        .find_map(|line| line.strip_prefix("Icon=").map(str::to_owned));
                    if icon.is_some() {
                        break;
                    }
                }
            }
            let icon = icon.unwrap_or_else(|| id.to_owned());
            let mut found = None;
            if Path::new(&icon).is_absolute() {
                found = Some(PathBuf::from(&icon));
            } else if !icon.contains('/') {
                found = themed_icon(&roots, &icon);
            }
            let file = found
                .filter(|p| mime_for(&p.to_string_lossy()).is_some_and(|m| m.starts_with("image/")))
                .ok_or_else(|| not_found("AssetWorkspaceAssetNotFoundError"))?;
            let real = std::fs::canonicalize(file)
                .map_err(|_| not_found("AssetWorkspaceAssetNotFoundError"))?;
            if protected_path(s, &real) || !icon_allowed(&real) {
                return Err(not_found("AssetWorkspaceAssetNotFoundError"));
            }
            let (c, name, rev) = media_claims(&real, false, expires)
                .ok_or_else(|| not_found("AssetWorkspaceAssetNotFoundError"))?;
            claims = c;
            claims["icon"] = json!(true);
            file_name = name;
            revision = Some(rev);
            dims = image_dimensions(&real);
        }
        "project-favicon" => {
            let cwd = text("cwd", 1024).ok_or_else(|| invalid("cwd is required"))?;
            let real_root = real_dir(Path::new(&cwd))
                .ok_or_else(|| not_found("AssetWorkspaceRootNormalizationError"))?;
            let requested = resource["path"].as_str().filter(|p| !p.is_empty());
            if requested.is_some_and(|p| !favicon_ext_ok(p)) {
                return Err(not_found("AssetPreviewTypeValidationError"));
            }
            let found = match requested {
                Some(p) => Some(
                    workspace_file(&real_root, p)
                        .ok_or_else(|| not_found("AssetProjectFaviconNotFoundError"))?,
                ),
                None => favicon_candidates()
                    .iter()
                    .find_map(|c| workspace_file(&real_root, c)),
            };
            let now = now_ms();
            expires = (now / FAVICON_BUCKET_MS + 2) * FAVICON_BUCKET_MS;
            match found {
                Some((real, rel)) => {
                    allowed_file(s, &real)
                        .ok_or_else(|| not_found("AssetWorkspacePathValidationError"))?;
                    source = Some(rel.clone());
                    let digest = sha256_file(&real).map_err(|e| json!({"_tag":"AssetProjectFaviconInspectionError","resource":resource,"cause":e.to_string()}))?;
                    file_name = format!(
                        "v{}-{}",
                        digest
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>(),
                        real.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    );
                    claims = json!({"version":1,"kind":"project-favicon","workspaceRoot":real_root.to_string_lossy(),"relativePath":rel,"expiresAt":expires});
                }
                None => {
                    file_name = MARKER.into();
                    claims = json!({"version":1,"kind":"project-favicon","workspaceRoot":real_root.to_string_lossy(),"relativePath":null,"expiresAt":expires});
                }
            }
        }
        "tool-output-image" => {
            let (tid, item, index) = (
                text("threadId", 256),
                text("itemId", 256),
                resource["index"].as_u64(),
            );
            let (Some(tid), Some(item), Some(index)) = (tid, item, index) else {
                return Err(invalid("threadId, itemId and index are required"));
            };
            let image = tool_output_image(s, &tid, &item, index as usize)
                .ok_or_else(|| not_found("AssetWorkspaceAssetNotFoundError"))?;
            dims = image_dimensions_bytes(&image.1);
            file_name = format!(
                "image-{}.{}",
                index + 1,
                image.0.trim_start_matches("image/")
            );
            claims = json!({"version":1,"kind":"tool-output-image","threadId":tid,"itemId":item,"index":index,"expiresAt":expires});
        }
        other => {
            return Err(
                json!({"_tag":"AssetWorkspaceResolutionError","resource":resource,"cause":format!("o3-server does not serve {other} assets")}),
            )
        }
    }
    if let Some(root) = claims["workspaceRoot"].as_str() {
        // Even draft cwd roots must refer to a registered workspace.
        if allowed_file(s, &Path::new(root).join(".")).is_none() {
            let db = s.db.lock().unwrap();
            let known: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE kind IN ('project','thread') AND (json_extract(body,'$.workspaceRoot')=?1 OR json_extract(body,'$.worktreePath')=?1))", [root], |r| r.get(0)).unwrap_or(false);
            if !known {
                return Err(invalid("unregistered workspace"));
            }
        }
    }
    claims["sessionId"] = p["_assetSession"].clone();
    claims["requiredScope"] = json!(required_scope(p));
    claims["expiresAt"] = json!(expires);
    let token = sign(s, &claims).map_err(
        |e| json!({"_tag":"AssetSigningKeyLoadError","resource":resource,"cause":e.to_string()}),
    )?;
    let mut out = json!({"relativeUrl":format!("{ASSET_PREFIX}{token}/{}",url::form_urlencoded::byte_serialize(file_name.as_bytes()).collect::<String>().replace('+', "%20")),"expiresAt":expires});
    if let Some(v) = source.take() {
        out["sourcePath"] = json!(v);
    }
    if let Some(v) = dims.take() {
        out["imageDimensions"] = v;
    }
    if let Some(v) = revision.take() {
        out["fileRevision"] = json!(v);
    }
    Ok(out)
}

/// Decoded bytes of image `index` in a tool-output turn item (`output.images[].data` is base64).
fn tool_output_image(
    s: &State,
    thread: &str,
    item: &str,
    index: usize,
) -> Option<(String, Vec<u8>)> {
    let db = s.db.lock().unwrap();
    let rec = events::get(&db, "turnItems", item).ok()?;
    if rec["threadId"].as_str().is_some_and(|t| t != thread) {
        return None;
    }
    let out = &rec["output"];
    let images = out["images"].as_array().or_else(|| out.as_array())?;
    let img = images.get(index)?;
    let data = img["data"]
        .as_str()
        .filter(|d| d.len() <= 14 * 1024 * 1024)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some((
        img["mimeType"].as_str().unwrap_or("image/png").to_owned(),
        bytes,
    ))
}
// ------------------------------------------------------------------ GET /api/assets/<token>/<name>

enum Resolved {
    File {
        path: PathBuf,
        download: bool,
        file_name: Option<String>,
        mime: Option<String>,
        identity: Option<(u64, u64, Option<String>)>,
        root: Option<PathBuf>,
    },
    Bytes {
        bytes: Vec<u8>,
        mime: String,
    },
}

fn resolve(s: &State, token: &str, rest: &str) -> Option<Resolved> {
    let c = verify(s, token, |_| true)?;
    let expected = if c["kind"] == "tool-output-image" {
        "orchestration:read"
    } else {
        "filesystem:read"
    };
    if c["requiredScope"] != expected {
        return None;
    }
    if let Some(sid) = c["sessionId"].as_str() {
        let scopes: String =
            s.db.lock()
                .ok()?
                .query_row(
                    "SELECT scopes FROM auth_sessions WHERE session_id=?",
                    [sid],
                    |r| r.get(0),
                )
                .ok()?;
        if !scopes.split_whitespace().any(|v| v == expected) {
            return None;
        }
    }
    let decoded = percent_decode(rest)?;
    let file = |path: PathBuf, root: Option<PathBuf>| {
        Some(Resolved::File {
            path,
            download: false,
            file_name: None,
            mime: None,
            identity: None,
            root,
        })
    };
    match c["kind"].as_str()? {
        "attachment" => {
            let path = attachment_path(s, c["attachmentId"].as_str()?)?;
            allowed_file(s, &path)?;
            Some(Resolved::File {
                path,
                download: c["download"] == true,
                file_name: c["fileName"].as_str().map(str::to_owned),
                mime: c["mimeType"].as_str().map(str::to_owned),
                identity: None,
                root: Some(attachments_dir(s)),
            })
        }
        "project-favicon" => {
            let root = c["workspaceRoot"].as_str()?;
            let (real, _) = workspace_file(Path::new(root), c["relativePath"].as_str()?)?;
            allowed_file(s, &real)?;
            file(real, real_dir(Path::new(root)))
        }
        "media-file-exact" => {
            let fp = PathBuf::from(c["filePath"].as_str()?);
            if decoded != fp.file_name()?.to_string_lossy() {
                return None;
            }
            let real = std::fs::canonicalize(&fp).ok().filter(|r| *r == fp)?;
            if c["icon"] == true {
                if protected_path(s, &real) {
                    return None;
                }
                if !icon_allowed(&real) || !favicon_ext_ok(&real.to_string_lossy()) {
                    return None;
                }
            } else if !media_allowed(s, &real) {
                return None;
            }
            let download = c["download"] == true;
            if !download && !previewable(&real.to_string_lossy()) {
                return None;
            }
            let identity = Some((
                c["device"].as_str()?.parse().ok()?,
                c["inode"].as_str()?.parse().ok()?,
                c["fileRevision"].as_str().map(str::to_owned),
            ));
            Some(Resolved::File {
                path: real.clone(),
                download,
                file_name: download
                    .then(|| real.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .flatten(),
                mime: None,
                identity,
                root: None,
            })
        }
        "workspace-file-exact" => {
            let rel = c["relativePath"].as_str()?;
            if decoded != Path::new(rel).file_name()?.to_string_lossy() {
                return None;
            }
            let root = c["workspaceRoot"].as_str()?;
            let (real, _) = workspace_file(Path::new(root), rel)?;
            allowed_file(s, &real)?;
            file(real, real_dir(Path::new(root)))
        }
        "workspace-file" => {
            let base = c["baseRelativePath"].as_str()?;
            let segments: Vec<&str> = decoded.split(['/', '\\']).collect();
            let ext = decoded
                .rsplit_once('.')
                .map(|(_, e)| e.to_lowercase())
                .unwrap_or_default();
            let mime_ext_ok = previewable(&decoded) || DEPENDENCY_EXT.contains(&ext.as_str());
            if decoded.is_empty()
                || decoded.contains('\0')
                || segments
                    .iter()
                    .any(|g| *g == "." || *g == ".." || g.starts_with('.') || g.is_empty())
                || !mime_ext_ok
            {
                return None;
            }
            let joined = if base == "." {
                decoded.clone()
            } else {
                format!("{base}/{decoded}")
            };
            let root = c["workspaceRoot"].as_str()?;
            let (real, _) = workspace_file(Path::new(root), &joined)?;
            allowed_file(s, &real)?;
            file(real, real_dir(Path::new(root)))
        }
        "tool-output-image" => {
            let (mime, bytes) = tool_output_image(
                s,
                c["threadId"].as_str()?,
                c["itemId"].as_str()?,
                c["index"].as_u64()? as usize,
            )?;
            Some(Resolved::Bytes { bytes, mime })
        }
        _ => None,
    }
}

pub(crate) fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let h = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(crate::auth::decode_hex(h)?[0]);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn disposition(name: Option<&str>) -> String {
    let Some(name) = name else {
        return "attachment".into();
    };
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || c == '"' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let ascii: String = clean
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '_' })
        .collect();
    if ascii == clean {
        format!("attachment; filename=\"{ascii}\"")
    } else {
        let enc: String = url::form_urlencoded::byte_serialize(clean.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
    }
}

fn byte_range(header: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = header
        .trim()
        .get(..6)
        .filter(|p| p.eq_ignore_ascii_case("bytes="))?;
    let rest = &header.trim()[spec.len()..];
    let (a, b) = rest.split_once('-')?;
    if a.is_empty() && b.is_empty() || rest.contains(',') {
        return None;
    }
    let (first, last) = (
        a.parse::<u64>().ok().filter(|_| !a.is_empty()),
        b.parse::<u64>().ok().filter(|_| !b.is_empty()),
    );
    if (!a.is_empty() && first.is_none()) || (!b.is_empty() && last.is_none()) {
        return None;
    }
    if first.zip(last).is_some_and(|(f, l)| l < f) {
        return None;
    }
    if size == 0 || first.is_some_and(|f| f >= size) || (first.is_none() && last == Some(0)) {
        return Some(Err(()));
    }
    let start = first.unwrap_or_else(|| size.saturating_sub(last.unwrap()));
    let end = match (first, last) {
        (Some(_), Some(l)) if l < size => l,
        _ => size - 1,
    };
    Some(Ok((start, end)))
}

/// `GET|HEAD /api/assets/<token>/<name>`: signed, unauthenticated, streamed, ranged.
pub async fn serve(s: &State, request: &Request<Incoming>) -> Response<Body> {
    let suffix = &request.uri().path()[ASSET_PREFIX.len()..];
    let Some((token, rest)) = suffix.split_once('/').filter(|(t, _)| !t.is_empty()) else {
        return text(StatusCode::NOT_FOUND, "Not Found");
    };
    let head = request.method() == hyper::Method::HEAD;
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let if_range = request.headers().contains_key(header::IF_RANGE);
    let (token, rest) = (token.to_owned(), rest.to_owned());
    let Some(resolved) = resolve(s, &token, &rest) else {
        return text(StatusCode::NOT_FOUND, "Not Found");
    };
    let (path, download, file_name, mime, identity, root) = match resolved {
        Resolved::Bytes { bytes, mime } => {
            let mut r = Response::new(if head {
                body::empty()
            } else {
                body::full(bytes.clone())
            });
            let h = r.headers_mut();
            h.insert(
                header::CONTENT_TYPE,
                mime.parse()
                    .unwrap_or(header::HeaderValue::from_static("application/octet-stream")),
            );
            h.insert(header::CONTENT_LENGTH, bytes.len().into());
            h.insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("private, max-age=3600"),
            );
            h.insert(
                "x-content-type-options",
                header::HeaderValue::from_static("nosniff"),
            );
            return r;
        }
        Resolved::File {
            path,
            download,
            file_name,
            mime,
            identity,
            root,
        } => (path, download, file_name, mime, identity, root),
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o4000)
        .open(&path)
    {
        Ok(f) => f,
        Err(_) => return text(StatusCode::NOT_FOUND, "Not Found"),
    };
    let Ok(meta) = file.metadata() else {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error");
    };
    if !meta.is_file()
        || identity.is_some_and(|(d, i, revision)| {
            (meta.dev(), meta.ino()) != (d, i)
                || revision.is_some_and(|expected| media_revision(&meta) != expected)
        })
    {
        return text(StatusCode::NOT_FOUND, "Not Found");
    }
    if let Some(root) = root {
        use std::os::fd::AsRawFd;
        // A file swapped for a link after resolution must still be inside its root.
        if std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .is_ok_and(|now| !paths::within(&root, &now))
        {
            return text(StatusCode::NOT_FOUND, "Not Found");
        }
    }
    let size = meta.len();
    let path_str = path.to_string_lossy().to_lowercase();
    let mut r = Response::new(body::empty());
    let content_type = if download {
        mime.as_deref()
            .filter(|m| safe_download_mime(m))
            .unwrap_or("application/octet-stream")
            .to_owned()
    } else {
        match mime
            .as_deref()
            .map(|m| m.split(';').next().unwrap_or("").trim().to_lowercase())
        {
            Some(m) if m == "text/html" => "text/html; charset=utf-8".into(),
            Some(m)
                if m == "application/pdf"
                    || ((m.starts_with("audio/") || m.starts_with("video/"))
                        && m.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"/!#$&^.+-_".contains(&b))) =>
            {
                m
            }
            _ if path_str.ends_with(".html") || path_str.ends_with(".htm") => {
                "text/html; charset=utf-8".into()
            }
            _ => mime_for(&path_str)
                .unwrap_or("application/octet-stream")
                .to_owned(),
        }
    };
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    h.insert(
        "x-content-type-options",
        header::HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    let media = content_type.starts_with("audio/") || content_type.starts_with("video/");
    h.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(if media {
            "private, no-store"
        } else {
            "private, max-age=3600"
        }),
    );
    if download {
        h.insert(
            header::CONTENT_DISPOSITION,
            disposition(file_name.as_deref())
                .parse()
                .unwrap_or(header::HeaderValue::from_static("attachment")),
        );
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static("default-src 'none'; sandbox"),
        );
    } else if content_type.starts_with("text/html") {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static(
                "sandbox allow-scripts allow-forms allow-popups allow-downloads",
            ),
        );
    } else if path_str.ends_with(".svg") {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static(
                "default-src 'none'; style-src 'unsafe-inline'; sandbox",
            ),
        );
    }
    let (mut start, mut len) = (0, size);
    if !head && !if_range {
        if let Some(Some(range)) = range.as_deref().map(|r| byte_range(r, size)) {
            match range {
                Err(()) => {
                    *r.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                    r.headers_mut().insert(
                        header::CONTENT_RANGE,
                        format!("bytes */{size}").parse().unwrap(),
                    );
                    return r;
                }
                Ok((a, b)) => {
                    *r.status_mut() = StatusCode::PARTIAL_CONTENT;
                    r.headers_mut().insert(
                        header::CONTENT_RANGE,
                        format!("bytes {a}-{b}/{size}").parse().unwrap(),
                    );
                    (start, len) = (a, b - a + 1);
                }
            }
        }
    }
    r.headers_mut().insert(header::CONTENT_LENGTH, len.into());
    if !head && len > 0 {
        *r.body_mut() = body::file_range(tokio::fs::File::from_std(file), start, len);
    }
    r
}

fn safe_download_mime(m: &str) -> bool {
    let m = m.trim().to_lowercase();
    let shape = m.split_once('/').is_some_and(|(a, b)| {
        !a.is_empty()
            && !b.is_empty()
            && m.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/!#$&^.+-_".contains(&c))
    });
    shape && m != "text/html" && !m.contains("/xml") && !m.ends_with("+xml")
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    #[test]
    fn profile_parent_stays_private_even_when_registered_as_a_workspace() {
        let base = std::env::temp_dir().join(random_id().unwrap());
        let mut s = State::open(&base.join("profile/userdata")).unwrap();
        s.asset_profile_root = base.join("profile");
        let private = base.join("profile/private.png");
        std::fs::write(&private, b"private").unwrap();
        s.db.lock()
            .unwrap()
            .execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES('project','p','',?)",
                [json!({"id":"p","workspaceRoot":base.join("profile")}).to_string()],
            )
            .unwrap();
        assert!(allowed_file(&s, &private).is_none());
        if let Some(home) = std::env::var_os("HOME") {
            for relative in [
                ".git-credentials",
                ".docker/config.json",
                ".kube/config",
                ".pypirc",
                ".local/share/keyrings/key",
                ".mozilla/profile",
                ".password-store/key",
                ".cargo/credentials.toml",
                ".bash_history",
            ] {
                assert!(
                    protected_path(&s, &PathBuf::from(&home).join(relative)),
                    "{relative}"
                );
            }
        }
        assert!(create_url(&s, &json!({"resource":{"_tag":"draft-workspace-file","cwd":base.join("profile"),"path":private}})).is_err());
        drop(s);
        // Fixture trees are retained for private container teardown.
    }
}

#[cfg(test)]
mod media_cache_tests {
    use super::*;
    #[test]
    fn icon_metadata_is_bounded_and_arbitrary_icon_directories_are_denied() {
        let root = std::env::temp_dir().join(random_id().unwrap());
        std::fs::create_dir_all(root.join("icons")).unwrap();
        let path = root.join("icons/large.png");
        std::fs::write(&path, vec![b'x'; 65537]).unwrap();
        assert!(bounded_text(&path).is_err());
        assert!(!icon_allowed(&path));
        assert!(bounded_text(&root).is_err());
        // Fixture trees are retained for private container teardown.
    }
    #[test]
    fn expired_cache_is_collected_and_media_types_follow_content() {
        let root = std::env::temp_dir().join(random_id().unwrap());
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("old.png");
        let live = root.join("live.mp4");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&live, b"live").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 3600))
            .unwrap();
        sweep_media(&root);
        assert!(!old.exists());
        assert!(live.exists());
        assert!(media_cache_has_room(&root));
        for i in 0..128 {
            std::fs::write(root.join(format!("{i}.png")), b"x").unwrap();
        }
        assert!(!media_cache_has_room(&root));
        assert_eq!(cached_media_extension(b"\xff\xd8\xffJPEG"), Some("jpg"));
        assert_eq!(cached_media_extension(b"\0\0\0\x18ftypisom"), Some("mp4"));
        assert_eq!(cached_media_extension(b"<html>login</html>"), None);
        // Fixture trees are retained for private container teardown.
    }
}
