//! Opt-in installed CLI check: cargo run --example live_attachment_intake --
//! claudeAgent|codex MODEL /path/to/o3-server . Uses a disposable profile.
#[path = "../tests/support/mod.rs"]
mod support;
use serde_json::json;
use std::{io::Write, sync::Arc, time::Duration};
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let instance = args.get(1).expect("provider required");
    assert!(matches!(instance.as_str(), "claudeAgent" | "codex"));
    let model = args.get(2).expect("model required");
    let worker = args.get(3).expect("worker binary required");
    let claude = o3_server::claude::ClaudeProvider::discover().map(Arc::new);
    if instance == "claudeAgent" {
        assert!(claude.is_some())
    }
    let codex = Arc::new(o3_server::codex::CodexProvider::new("codex".into(), None));
    let s = support::start(|s| {
        s.set_document_worker(worker.into());
        std::fs::write(
            s.dir.join("settings.json"),
            json!({"enableAgentBrowserAccess":false}).to_string(),
        )
        .unwrap();
        *s.provider.lock().unwrap() =
            Some(Arc::new(o3_server::provider::Providers { claude, codex }));
    });
    let root = s.dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let c = s.connect();
    c.rpc("projects.mutate",json!({"type":"project.create","commandId":"p","projectId":"p","title":"Document proof","workspaceRoot":root})).unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"Document proof","modelSelection":{"instanceId":instance,"model":model},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    let challenge = format!("DOCUMENT-{}", o3_server::store::random_id().unwrap());
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(
        "word/document.xml",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(format!("<w:document xmlns:w=\"urn:w\"><w:body><w:p><w:r><w:t>Write exactly this token as answer.txt: {challenge}</w:t></w:r></w:p></w:body></w:document>").as_bytes()).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let file = c
        .upload_attachment(
            "task.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            bytes.len() as u64,
            std::io::Cursor::new(bytes),
        )
        .unwrap();
    c.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"send","messageId":"m","threadId":"t","text":"Disposable document intake check. Read the text extracted from the attached document at the provided text-extracted path. Follow its instruction to write answer.txt in your current workspace, with no newline or other text. Finish after that. Do not launch other threads or delegates.","attachments":[file],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    c.rpc("attachments.delete", json!({"attachmentId":file["id"]}))
        .unwrap();
    let done = support::wait_for(Duration::from_secs(120), || {
        c.rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap()["runs"][0]["completedAt"]
            .is_string()
    });
    let p = c
        .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
        .unwrap();
    let content = std::fs::read_to_string(root.join("answer.txt")).unwrap_or_default();
    let copied = &p["messages"][0]["attachments"][0]["id"];
    let path = o3_server::assets::attachment_path(&s.state, copied.as_str().unwrap()).unwrap();
    let extracted = std::fs::read_to_string(format!("{}.txt", path.display())).unwrap();
    let exact = content == challenge;
    println!(
        "{}",
        json!({"provider":instance,"model":model,"completed":done,"status":p["runs"][0]["status"],"read_extracted_document_exactly":exact,"source_deleted_copy_survived":path.is_file(),"sidecar_contains_challenge":extracted.contains(&challenge)})
    );
    assert!(
        done && exact && p["runs"][0]["status"] == "completed",
        "{p}"
    );
}
