//! Conformance: attachments, signed assets, projects over HTTP, and workspace file RPCs.
mod common;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64},
    Engine,
};
use common::*;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

fn png() -> Vec<u8> {
    // 1x1 RGBA PNG.
    STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==").unwrap()
}
fn sign(env: &Env, mut claims: serde_json::Value) -> String {
    let key = std::fs::read(env.state.join("secrets/asset-access-signing-key.bin")).unwrap();
    claims["requiredScope"] = json!("filesystem:read");
    let payload = B64.encode(claims.to_string());
    let mut m = Hmac::<Sha256>::new_from_slice(&key).unwrap();
    m.update(payload.as_bytes());
    format!("{payload}.{}", B64.encode(m.finalize().into_bytes()))
}
fn far() -> i64 {
    i64::MAX / 4
}

#[test]
fn upload_streams_to_disk_and_asset_round_trips_through_the_client() {
    let e = setup("upload");
    let bytes = png();
    let r = e
        .conn
        .upload_attachment(
            "pixel.png",
            "image/png",
            bytes.len() as u64,
            std::io::Cursor::new(bytes.clone()),
        )
        .unwrap();
    let id = r["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("pending-"), "{id}");
    let stored = e.state.join("attachments").join(format!("{id}.png"));
    assert_eq!(std::fs::read(&stored).unwrap(), bytes);
    // assets.createUrl + signed GET, via the client's image reader (checks PNG magic and type).
    assert_eq!(
        e.conn
            .attachment_image(&id, "pixel.png", "image/png")
            .unwrap(),
        bytes
    );
    // The grant is exposed verbatim with dimensions.
    let grant = e.conn.rpc("assets.createUrl", json!({"resource":{"_tag":"attachment","attachmentId":id,"fileName":"pixel.png","mimeType":"image/png","disposition":"inline"}})).unwrap();
    assert_eq!(grant["imageDimensions"], json!({"width":1,"height":1}));
    let url = grant["relativeUrl"].as_str().unwrap();
    assert!(
        url.starts_with("/api/assets/") && url.ends_with("/pixel.png"),
        "{url}"
    );
    let (status, head, body) = http(&e.base, "GET", url, &[], b"");
    assert_eq!((status, body), (200, bytes.clone()));
    assert!(
        head.contains("content-type: image/png")
            && head.contains("x-content-type-options: nosniff"),
        "{head}"
    );
    // Delete removes the pending upload; the grant can no longer be minted.
    e.conn
        .rpc("attachments.delete", json!({"attachmentId":id}))
        .unwrap();
    assert!(!stored.exists());
    assert!(e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"attachment","attachmentId":id}})
        )
        .is_err());
    // Idempotent, and ids that are not pending uploads or not ids at all are ignored.
    e.conn
        .rpc("attachments.delete", json!({"attachmentId":id}))
        .unwrap();
    e.conn
        .rpc(
            "attachments.delete",
            json!({"attachmentId":"../../etc/passwd"}),
        )
        .unwrap();
}

#[test]
fn large_files_stream_and_ranges_are_served() {
    let e = setup("large");
    let size = 12 * 1024 * 1024 + 123;
    let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let r = e
        .conn
        .upload_attachment(
            "big.bin",
            "application/octet-stream",
            size as u64,
            std::io::Cursor::new(data.clone()),
        )
        .unwrap();
    let id = r["id"].as_str().unwrap();
    assert!(id.ends_with("-bin"), "{id}");
    assert_eq!(
        std::fs::read(e.state.join("attachments").join(format!("{id}.bin"))).unwrap(),
        data
    );
    let grant = e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"attachment","attachmentId":id,"fileName":"big.bin"}}),
        )
        .unwrap();
    let url = grant["relativeUrl"].as_str().unwrap();
    let (status, head, body) = http(&e.base, "GET", url, &[("Range", "bytes=1000-1999")], b"");
    assert_eq!(status, 206, "{head}");
    assert!(
        head.contains(&format!("content-range: bytes 1000-1999/{size}")),
        "{head}"
    );
    assert_eq!(body, data[1000..2000]);
    // Generic files are downloads, not inline documents.
    assert!(
        head.contains("content-disposition: attachment; filename=\"big.bin\""),
        "{head}"
    );
    let (status, _, body) = http(&e.base, "GET", url, &[("Range", "bytes=-5")], b"");
    assert_eq!((status, body.as_slice()), (206, &data[size - 5..]));
    let (status, head, _) = http(
        &e.base,
        "GET",
        url,
        &[("Range", &format!("bytes={size}-"))],
        b"",
    );
    assert_eq!(status, 416, "{head}");
    let (status, head, body) = http(&e.base, "HEAD", url, &[], b"");
    assert!(
        status == 200 && body.is_empty() && head.contains(&format!("content-length: {size}")),
        "{head}"
    );
}

#[test]
fn upload_rejects_bad_tokens_sizes_and_oversize_declarations() {
    let e = setup("upload-bad");
    let grant = e
        .conn
        .rpc(
            "attachments.createUploadUrl",
            json!({"type":"file","name":"n.txt","mimeType":"text/plain","sizeBytes":5}),
        )
        .unwrap();
    let url = grant["relativeUrl"].as_str().unwrap();
    assert!(url.starts_with("/api/attachments/upload/"));
    // No credential header is needed, the URL is the grant.
    let (status, _, _) = http(&e.base, "POST", url, &[("Content-Length", "4")], b"abcd");
    assert_eq!(status, 400);
    let (status, _, _) = http(
        &e.base,
        "POST",
        url,
        &[("Transfer-Encoding", "chunked")],
        b"A\r\n0123456789\r\n0\r\n\r\n",
    );
    assert_eq!(
        status, 400,
        "a chunked body longer than declared must be refused"
    );
    assert!(std::fs::read_dir(e.state.join("attachments"))
        .unwrap()
        .all(|f| !f.unwrap().file_name().to_string_lossy().ends_with(".part")));
    let tampered = format!("{}x", url);
    assert_eq!(http(&e.base, "POST", &tampered, &[], b"hello").0, 404);
    assert_eq!(
        http(&e.base, "POST", "/api/attachments/upload/", &[], b"hello").0,
        404
    );
    assert_eq!(http(&e.base, "POST", url, &[], b"hello").0, 204);
    // Re-sending identical bytes is fine; different bytes never replace the stored upload.
    assert_eq!(http(&e.base, "POST", url, &[], b"hello").0, 204);
    assert_eq!(http(&e.base, "POST", url, &[], b"world").0, 409);
    let stored = grant["attachmentId"].as_str().unwrap();
    assert_eq!(
        std::fs::read(e.state.join("attachments").join(format!("{stored}.txt"))).unwrap(),
        b"hello"
    );
    // Declarations beyond T3's limits are refused when minting.
    assert!(e
        .conn
        .rpc(
            "attachments.createUploadUrl",
            json!({"type":"image","name":"a.png","mimeType":"image/png","sizeBytes":10*1024*1024+1})
        )
        .is_err());
    assert!(e
        .conn
        .rpc(
            "attachments.createUploadUrl",
            json!({"type":"file","name":"a.bin","mimeType":"x/y","sizeBytes":50*1024*1024+1})
        )
        .is_err());
    assert!(e
        .conn
        .rpc(
            "attachments.createUploadUrl",
            json!({"type":"file","name":"a.bin","mimeType":"x/y","sizeBytes":0})
        )
        .is_err());
    // An expired grant is rejected even with a valid signature.
    let expired = sign(
        &e,
        json!({"version":1,"kind":"attachment-upload","type":"file","attachmentId":"pending-00000000-0000-4000-8000-000000000000-txt","name":"n.txt","mimeType":"text/plain","sizeBytes":1,"expiresAt":1}),
    );
    assert_eq!(
        http(
            &e.base,
            "POST",
            &format!("/api/attachments/upload/{expired}"),
            &[],
            b"x"
        )
        .0,
        404
    );
}

#[test]
fn workspace_assets_are_signed_confined_and_path_traversal_safe() {
    let e = setup("assets");
    let root = e.dir("proj");
    let outside = e.dir("outside");
    let pic = png();
    write(&root.join("img/pic.png"), &pic);
    write(&root.join("page/index.html"), "<img src=\"a.png\">");
    write(&root.join("page/a.png"), &pic);
    write(&root.join("page/.hidden.png"), &pic);
    write(&root.join("notes.txt"), "plain text");
    write(&outside.join("secret.png"), &pic);
    std::os::unix::fs::symlink(outside.join("secret.png"), root.join("img/link.png")).unwrap();
    e.project("p1", &root);
    e.thread("t1", "p1");
    // The client's own image reader for host media.
    let (bytes, mime) = e.conn.media_image("t1", "img/pic.png").unwrap();
    assert_eq!((bytes, mime.as_str()), (pic.clone(), "image/png"));
    // workspace-file grants.
    let grant = e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"workspace-file","threadId":"t1","path":"notes.txt"}}),
        )
        .unwrap();
    let (status, _, body) = http(
        &e.base,
        "GET",
        grant["relativeUrl"].as_str().unwrap(),
        &[],
        b"",
    );
    assert_eq!((status, body.as_slice()), (200, &b"plain text"[..]));
    // Escapes are refused when minting, whatever their spelling.
    for bad in [
        "../outside/secret.png",
        "img/../../outside/secret.png",
        "img/link.png",
        "missing.png",
    ] {
        assert!(
            e.conn
                .rpc(
                    "assets.createUrl",
                    json!({"resource":{"_tag":"workspace-file","threadId":"t1","path":bad}})
                )
                .is_err(),
            "{bad}"
        );
    }
    assert!(e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"workspace-file","threadId":"nope","path":"notes.txt"}})
        )
        .is_err());
    // An unsupported preview type is not signed.
    write(&root.join("tool.exe"), "MZ");
    assert!(e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"workspace-file","threadId":"t1","path":"tool.exe"}})
        )
        .is_err());
    // An HTML grant also serves its dependencies, but never `..`, dotfiles or other directories.
    let html = e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"workspace-file","threadId":"t1","path":"page/index.html"}}),
        )
        .unwrap();
    let url = html["relativeUrl"].as_str().unwrap();
    let token = url
        .trim_start_matches("/api/assets/")
        .split('/')
        .next()
        .unwrap();
    let (status, head, _) = http(&e.base, "GET", url, &[], b"");
    assert!(
        status == 200 && head.contains("content-security-policy: sandbox"),
        "{head}"
    );
    assert_eq!(
        http(
            &e.base,
            "GET",
            &format!("/api/assets/{token}/a.png"),
            &[],
            b""
        )
        .0,
        200
    );
    for bad in [
        ".hidden.png",
        "..%2Fnotes.txt",
        "%2e%2e/notes.txt",
        "../notes.txt",
        "..%2F..%2Foutside%2Fsecret.png",
        "a.png%00.html",
        "sub/../a.png",
        "%2fetc%2fpasswd",
        "missing.png",
    ] {
        assert_eq!(
            http(
                &e.base,
                "GET",
                &format!("/api/assets/{token}/{bad}"),
                &[],
                b""
            )
            .0,
            404,
            "{bad}"
        );
    }
    // Forged or altered tokens: wrong key, edited claims, extra segments, expiry.
    let claims = json!({"version":1,"kind":"workspace-file","workspaceRoot":root.to_str().unwrap(),"baseRelativePath":"page","expiresAt":far()});
    let good = sign(&e, claims.clone());
    assert_eq!(
        http(
            &e.base,
            "GET",
            &format!("/api/assets/{good}/a.png"),
            &[],
            b""
        )
        .0,
        200,
        "tokens signed with the secrets key are accepted"
    );
    let (payload, sig) = good.split_once('.').unwrap();
    let swapped = format!(
        "{}.{sig}",
        B64.encode(claims.to_string().replace("page", "img"))
    );
    assert_eq!(
        http(
            &e.base,
            "GET",
            &format!("/api/assets/{swapped}/pic.png"),
            &[],
            b""
        )
        .0,
        404
    );
    assert_eq!(
        http(
            &e.base,
            "GET",
            &format!("/api/assets/{payload}.{}/a.png", B64.encode(b"nope")),
            &[],
            b""
        )
        .0,
        404
    );
    assert_eq!(
        http(
            &e.base,
            "GET",
            &format!("/api/assets/{good}.extra/a.png"),
            &[],
            b""
        )
        .0,
        404
    );
    let expired = sign(
        &e,
        json!({"version":1,"kind":"workspace-file","workspaceRoot":root.to_str().unwrap(),"baseRelativePath":"page","expiresAt":1}),
    );
    assert_eq!(
        http(
            &e.base,
            "GET",
            &format!("/api/assets/{expired}/a.png"),
            &[],
            b""
        )
        .0,
        404
    );
    // A signed claim cannot be stretched beyond its root with a symlink or an absolute name.
    let link_claims = json!({"version":1,"kind":"workspace-file","workspaceRoot":root.to_str().unwrap(),"baseRelativePath":"img","expiresAt":far()});
    assert_eq!(
        http(
            &e.base,
            "GET",
            &format!("/api/assets/{}/link.png", sign(&e, link_claims)),
            &[],
            b""
        )
        .0,
        404,
        "symlink out of the root"
    );
    let exact = json!({"version":1,"kind":"workspace-file-exact","workspaceRoot":root.to_str().unwrap(),"relativePath":"../outside/secret.png","expiresAt":far()});
    assert_eq!(
        http(
            &e.base,
            "GET",
            &format!("/api/assets/{}/secret.png", sign(&e, exact)),
            &[],
            b""
        )
        .0,
        404
    );
    assert_eq!(http(&e.base, "GET", "/api/assets/", &[], b"").0, 404);
    assert_eq!(
        http(&e.base, "GET", "/api/assets/onlytoken", &[], b"").0,
        404
    );
    // media-file grants pin the file's identity: replacing it invalidates the URL.
    let media = e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"media-file","threadId":"t1","path":"img/pic.png"}}),
        )
        .unwrap();
    let murl = media["relativeUrl"].as_str().unwrap();
    assert!(media["fileRevision"].is_string());
    assert_eq!(http(&e.base, "GET", murl, &[], b"").0, 200);
    std::fs::remove_file(root.join("img/pic.png")).unwrap();
    write(&root.join("img/pic.png"), &pic);
    assert_eq!(http(&e.base, "GET", murl, &[], b"").0, 404);
    // project favicon: found, and the "missing" sentinel when absent.
    write(&root.join("favicon.png"), &pic);
    let fav = e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"project-favicon","cwd":root.to_str().unwrap()}}),
        )
        .unwrap();
    assert_eq!(fav["sourcePath"], "favicon.png");
    assert_eq!(
        http(
            &e.base,
            "GET",
            fav["relativeUrl"].as_str().unwrap(),
            &[],
            b""
        )
        .0,
        200
    );
    let empty = e.dir("emptyproj");
    e.project("empty", &empty);
    let none = e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"project-favicon","cwd":empty.to_str().unwrap()}}),
        )
        .unwrap();
    assert!(none["relativeUrl"]
        .as_str()
        .unwrap()
        .ends_with("/project-favicon-missing"));
    assert_eq!(
        http(
            &e.base,
            "GET",
            none["relativeUrl"].as_str().unwrap(),
            &[],
            b""
        )
        .0,
        404
    );
}

#[test]
fn projects_over_http_publish_shell_events_and_validate_roots() {
    let e = setup("projects");
    let root = e.dir("p");
    let other = e.dir("q");
    let snap = e
        .conn
        .request_json("GET", "/api/projects", None, false)
        .unwrap();
    assert_eq!(snap["projects"], json!([]));
    assert!(snap["updatedAt"].is_string());
    let create = json!({"type":"project.create","commandId":"c1","projectId":"p1","title":"One","workspaceRoot":root.to_str().unwrap(),"scripts":[]});
    let project = e
        .conn
        .request_json("POST", "/api/projects/mutate", Some(&create), false)
        .unwrap();
    assert_eq!(
        (project["id"].as_str(), project["title"].as_str()),
        (Some("p1"), Some("One"))
    );
    // Replays are idempotent; the same root cannot back two projects; missing roots are refused.
    assert_eq!(
        e.conn
            .request_json("POST", "/api/projects/mutate", Some(&create), false)
            .unwrap(),
        project
    );
    let clash = json!({"type":"project.create","commandId":"c2","projectId":"p2","title":"Two","workspaceRoot":root.to_str().unwrap(),"scripts":[]});
    assert!(e
        .conn
        .request_json("POST", "/api/projects/mutate", Some(&clash), false)
        .is_err());
    let missing = json!({"type":"project.create","commandId":"c3","projectId":"p3","title":"Three","workspaceRoot":e.scratch.join("nope").to_str().unwrap(),"scripts":[]});
    assert!(e
        .conn
        .request_json("POST", "/api/projects/mutate", Some(&missing), false)
        .is_err());
    let made = e.scratch.join("made/deep");
    let created = json!({"type":"project.create","commandId":"c4","projectId":"p4","title":"Four","workspaceRoot":made.to_str().unwrap(),"createWorkspaceRootIfMissing":true,"scripts":[]});
    e.conn.rpc("projects.mutate", created).unwrap();
    assert!(made.is_dir());
    // update and delete go through the RPC and publish shell events on the shared sequence.
    let updated = e.conn.rpc("projects.mutate", json!({"type":"project.update","commandId":"u1","projectId":"p1","title":"Renamed","workspaceRoot":other.to_str().unwrap()})).unwrap();
    assert_eq!(
        (updated["title"].as_str(), updated["workspaceRoot"].as_str()),
        (Some("Renamed"), other.to_str())
    );
    e.thread("t1", "p1");
    assert!(
        e.conn
            .rpc(
                "projects.mutate",
                json!({"type":"project.delete","commandId":"d0","projectId":"p1"})
            )
            .is_err(),
        "a project with threads needs force"
    );
    e.conn
        .rpc(
            "projects.mutate",
            json!({"type":"project.delete","commandId":"d1","projectId":"p4"}),
        )
        .unwrap();
    let list = e
        .conn
        .request_json("GET", "/api/projects", None, false)
        .unwrap();
    let ids: Vec<_> = list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["p1"]);
    let items = collect(
        &e.conn,
        "orchestration.subscribeShell",
        json!({"afterSequence":0,"requestCompletionMarker":true}),
        |v| v["kind"] == "synchronized",
    );
    assert!(items
        .iter()
        .any(|v| v["kind"] == "project.updated" && v["project"]["id"] == "p1"));
    assert!(
        items
            .iter()
            .any(|v| v["kind"] == "project.removed" && v["projectId"] == "p4"
                || v["kind"] == "snapshot"
                    && v["snapshot"]["projects"]
                        .as_array()
                        .is_some_and(|rows| rows.iter().all(|p| p["id"] != "p4"))),
        "{items:?}"
    );
    // Reads need orchestration:read, mutations orchestration:operate.
    let grant = e
        .conn
        .request_json(
            "POST",
            "/api/auth/pairing-token",
            Some(&json!({"scopes":["orchestration:read"]})),
            false,
        )
        .unwrap();
    let paired = t3_protocol::pair_bearer(
        &e.base,
        grant["credential"].as_str().unwrap(),
        None,
        &t3_protocol::PairingClient::default(),
        &t3_protocol::ConnectionOptions::default(),
    )
    .unwrap();
    let ro = paired
        .connection(&t3_protocol::ConnectionOptions::default())
        .unwrap();
    assert!(ro.request_json("GET", "/api/projects", None, false).is_ok());
    assert!(ro
        .request_json(
            "POST",
            "/api/projects/mutate",
            Some(&json!({"type":"project.delete","commandId":"x","projectId":"p1","force":true})),
            false
        )
        .is_err());
}

#[test]
fn read_file_is_rooted_bounded_and_text_only() {
    let e = setup("read");
    let root = e.dir("w");
    let outside = e.dir("o");
    write(&root.join("src/a.txt"), "héllo\n");
    write(&root.join("big.txt"), "x".repeat(1024 * 1024 + 10));
    write(&root.join("bin.dat"), [1u8, 0, 2]);
    write(&outside.join("secret.txt"), "TOP SECRET");
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("dirlink")).unwrap();
    let cwd = root.to_str().unwrap();
    let r = e
        .conn
        .rpc(
            "projects.readFile",
            json!({"cwd":cwd,"relativePath":"src/a.txt"}),
        )
        .unwrap();
    assert_eq!(
        (
            r["contents"].as_str(),
            r["byteLength"].as_u64(),
            r["truncated"].as_bool()
        ),
        (Some("héllo\n"), Some(7), Some(false))
    );
    let big = e
        .conn
        .rpc(
            "projects.readFile",
            json!({"cwd":cwd,"relativePath":"big.txt"}),
        )
        .unwrap();
    assert_eq!(
        (
            big["contents"].as_str().unwrap().len(),
            big["truncated"].as_bool()
        ),
        (1024 * 1024, Some(true))
    );
    assert_eq!(big["byteLength"], 1024 * 1024 + 10);
    for (path, needle) in [
        ("../o/secret.txt", "must be relative to the project root"),
        (
            "src/../../o/secret.txt",
            "must be relative to the project root",
        ),
        ("link.txt", "resolves outside the project root"),
        ("dirlink/secret.txt", "resolves outside the project root"),
        ("bin.dat", "is binary"),
        ("src", "is not a file"),
        ("nope.txt", "No such file"),
        (".", "must be relative to the project root"),
    ] {
        let msg = err_text(
            e.conn
                .rpc("projects.readFile", json!({"cwd":cwd,"relativePath":path})),
        );
        assert!(msg.contains(needle), "{path}: {msg}");
    }
    let absent = e
        .conn
        .rpc(
            "projects.readFile",
            json!({"cwd":cwd,"relativePath":"nope.txt","allowMissing":true}),
        )
        .unwrap();
    assert_eq!(
        (absent["exists"].as_bool(), absent["contents"].as_str()),
        (Some(false), Some(""))
    );
    // A missing path below a link that leaves the root still counts as an escape.
    let msg = err_text(e.conn.rpc(
        "projects.readFile",
        json!({"cwd":cwd,"relativePath":"dirlink/new.txt","allowMissing":true}),
    ));
    assert!(msg.contains("resolves outside the project root"), "{msg}");
    // Absolute host paths are read in place, as T3 does.
    let abs = e
        .conn
        .rpc(
            "projects.readFile",
            json!({"cwd":cwd,"relativePath":outside.join("secret.txt").to_str().unwrap()}),
        )
        .unwrap();
    assert_eq!(abs["contents"], "TOP SECRET");
    // filesystem:read is required.
    let grant = e
        .conn
        .request_json(
            "POST",
            "/api/auth/pairing-token",
            Some(&json!({"scopes":["orchestration:read"]})),
            false,
        )
        .unwrap();
    let paired = t3_protocol::pair_bearer(
        &e.base,
        grant["credential"].as_str().unwrap(),
        None,
        &t3_protocol::PairingClient::default(),
        &t3_protocol::ConnectionOptions::default(),
    )
    .unwrap();
    let ro = paired
        .connection(&t3_protocol::ConnectionOptions::default())
        .unwrap();
    assert!(err_text(ro.rpc(
        "projects.readFile",
        json!({"cwd":cwd,"relativePath":"src/a.txt"})
    ))
    .contains("filesystem:read"));
}

#[test]
fn search_entries_browse_and_metadata() {
    let e = setup("search");
    let root = e.dir("repo");
    repo(&root);
    write(&root.join(".gitignore"), "ignored/\n");
    for f in [
        "src/main.rs",
        "src/lib.rs",
        "src/deep/mod_widget.rs",
        "README.md",
        "assets/logo.png",
        "ignored/skip.rs",
    ] {
        write(&root.join(f), "x");
    }
    let cwd = root.to_str().unwrap();
    let paths = |p: serde_json::Value| -> Vec<String> {
        e.conn.rpc("projects.searchEntries", p).unwrap()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["path"].as_str().unwrap().to_owned())
            .collect()
    };
    let r = paths(json!({"cwd":cwd,"query":"main","limit":10}));
    assert_eq!(r[0], "src/main.rs");
    assert!(
        !paths(json!({"cwd":cwd,"query":"skip","limit":10}))
            .contains(&"ignored/skip.rs".to_owned()),
        "ignore rules apply"
    );
    assert_eq!(
        paths(json!({"cwd":cwd,"query":"@src/deep","limit":10,"kind":"directory"})),
        ["src/deep"]
    );
    assert_eq!(
        paths(json!({"cwd":cwd,"query":"","limit":10,"imageOnly":true})),
        ["assets/logo.png"]
    );
    assert_eq!(
        paths(json!({"cwd":cwd,"query":"mw","limit":10}))[0],
        "src/deep/mod_widget.rs",
        "subsequence match"
    );
    let capped = e
        .conn
        .rpc(
            "projects.searchEntries",
            json!({"cwd":cwd,"query":"","limit":2}),
        )
        .unwrap();
    assert_eq!(
        (
            capped["entries"].as_array().unwrap().len(),
            capped["truncated"].as_bool()
        ),
        (2, Some(true))
    );
    assert!(err_text(e.conn.rpc(
        "projects.searchEntries",
        json!({"cwd":e.scratch.join("none").to_str().unwrap(),"query":"x","limit":5})
    ))
    .contains("Workspace root does not exist"));
    assert!(
        e.conn
            .rpc(
                "projects.searchEntries",
                json!({"cwd":cwd,"query":"x","limit":5000})
            )
            .is_ok(),
        "limits are clamped, not trusted"
    );
    // filesystem.browse
    let parent = e.dir("browse");
    for d in ["alpha", "Beta", ".hidden", "alphabet"] {
        std::fs::create_dir_all(parent.join(d)).unwrap();
    }
    write(&parent.join("file.txt"), "x");
    let names = |p: serde_json::Value| -> Vec<String> {
        e.conn.rpc("filesystem.browse", p).unwrap()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let base = format!("{}/", parent.to_str().unwrap());
    assert_eq!(
        names(json!({"partialPath":base})),
        [".hidden", "alpha", "alphabet", "Beta"],
        "a trailing slash lists hidden folders too"
    );
    assert_eq!(
        names(json!({"partialPath":format!("{}/al", parent.to_str().unwrap())})),
        ["alpha", "alphabet"]
    );
    assert_eq!(
        names(json!({"partialPath":format!("{}/.h", parent.to_str().unwrap())})),
        [".hidden"]
    );
    let rel = e
        .conn
        .rpc(
            "filesystem.browse",
            json!({"partialPath":"./al","cwd":parent.to_str().unwrap()}),
        )
        .unwrap();
    assert_eq!(rel["parentPath"], parent.to_str().unwrap());
    assert!(err_text(
        e.conn
            .rpc("filesystem.browse", json!({"partialPath":"./al"}))
    )
    .contains("A current project is required"));
    assert!(err_text(
        e.conn
            .rpc("filesystem.browse", json!({"partialPath":"C:\\Users\\x"}))
    )
    .contains("Windows paths are not supported"));
    assert!(err_text(e.conn.rpc(
        "filesystem.browse",
        json!({"partialPath":format!("{}/nope/x", parent.to_str().unwrap())})
    ))
    .contains("Failed to read workspace directory"));
    // filesystem.getMetadata
    let png_path = parent.join("pic");
    write(&png_path, png());
    let meta = e.conn.rpc("filesystem.getMetadata", json!({"paths":[parent.join("file.txt").to_str().unwrap(), parent.to_str().unwrap(), png_path.to_str().unwrap(), "relative/path", e.scratch.join("none").to_str().unwrap()]})).unwrap();
    assert_eq!(meta["entries"][0], json!({"kind":"file","byteLength":1}));
    assert_eq!(meta["entries"][1], json!({"kind":"directory"}));
    assert_eq!(meta["entries"][2]["mimeType"], "image/png");
    assert!(meta["entries"][3].is_null() && meta["entries"][4].is_null());
    assert!(e
        .conn
        .rpc("filesystem.getMetadata", json!({"paths":[]}))
        .is_err());
    let many: Vec<String> = (0..65).map(|i| format!("/x/{i}")).collect();
    assert!(e
        .conn
        .rpc("filesystem.getMetadata", json!({"paths":many}))
        .is_err());
}

#[test]
fn open_in_editor_launches_argv_without_a_shell() {
    let e = setup("editor");
    let target = e.dir("proj with spaces; touch pwned");
    let out = bin_dir().join("code.out");
    let _ = std::fs::remove_file(&out);
    assert!(e
        .conn
        .rpc(
            "shell.openInEditor",
            json!({"cwd":target.to_str().unwrap(),"editor":"vscode"})
        )
        .is_ok());
    let start = std::time::Instant::now();
    while !out.exists() && start.elapsed().as_secs() < 5 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        std::fs::read_to_string(&out).unwrap().trim(),
        target.to_str().unwrap()
    );
    assert!(!e.scratch.join("pwned").exists());
    assert!(e
        .conn
        .rpc(
            "shell.openInEditor",
            json!({"cwd":"-evil","editor":"vscode"})
        )
        .is_err());
    assert!(e
        .conn
        .rpc(
            "shell.openInEditor",
            json!({"cwd":target.to_str().unwrap(),"editor":"emacs; rm -rf /"})
        )
        .is_err());
    assert!(
        e.conn
            .rpc(
                "shell.openInEditor",
                json!({"cwd":target.to_str().unwrap(),"editor":"zed"})
            )
            .is_err(),
        "an editor that is not installed fails loudly"
    );
}

#[test]
fn file_asset_grants_require_filesystem_scope_and_registered_boundaries() {
    let e = setup("asset-boundaries");
    let root = e.dir("workspace");
    e.project("p", &root);
    e.thread("t", "p");
    write(&root.join("ok.png"), png());
    let outside = e.scratch.join("outside.png");
    write(&outside, png());
    std::os::unix::fs::symlink(&outside, root.join("escape.png")).unwrap();
    let grant = e
        .conn
        .request_json(
            "POST",
            "/api/auth/pairing-token",
            Some(&json!({"scopes":["orchestration:read"]})),
            false,
        )
        .unwrap();
    let ro = t3_protocol::pair_bearer(
        &e.base,
        grant["credential"].as_str().unwrap(),
        None,
        &Default::default(),
        &Default::default(),
    )
    .unwrap()
    .connection(&Default::default())
    .unwrap();
    for path in [
        outside,
        root.join("../outside.png"),
        root.join("escape.png"),
        e.state.join("secrets/server-signing-key.bin"),
        e.state.join("o3.sqlite"),
    ] {
        for tag in ["media-file", "workspace-file", "draft-workspace-file"] {
            let p = json!({"resource":{"_tag":tag,"threadId":"t","cwd":root,"path":path,"disposition":"attachment"}});
            assert!(ro.rpc("assets.createUrl", p.clone()).is_err(), "{p}");
            assert!(e.conn.rpc("assets.createUrl", p).is_err(), "{tag} {path:?}");
        }
        assert!(ro
            .rpc(
                "assets.createUrl",
                json!({"resource":{"_tag":"attachment","attachmentId":path}})
            )
            .is_err());
    }
    assert!(ro.rpc("assets.createUrl",json!({"resource":{"_tag":"media-file","threadId":"t","path":"ok.png","disposition":"attachment"}})).is_err());
    let good = e.conn.rpc("assets.createUrl",json!({"resource":{"_tag":"media-file","threadId":"t","path":"ok.png","disposition":"attachment"}})).unwrap();
    assert_eq!(
        http(
            &e.base,
            "GET",
            good["relativeUrl"].as_str().unwrap(),
            &[],
            b""
        )
        .0,
        200
    );
    std::fs::remove_file(root.join("ok.png")).unwrap();
    std::os::unix::fs::symlink(e.state.join("o3.sqlite"), root.join("ok.png")).unwrap();
    assert_eq!(
        http(
            &e.base,
            "GET",
            good["relativeUrl"].as_str().unwrap(),
            &[],
            b""
        )
        .0,
        404
    );
}
