//! Dispatch for RPCs that need I/O or subprocesses. `rpc.rs` runs these on spawned tasks so a slow
//! `git pull` never stalls the rest of a socket.
use crate::{assets, files, review, store::State, threadq, vcs};
use serde_json::Value;

pub const STREAM_ACTION: &str = "git.runStackedAction";

pub fn handles(tag: &str) -> bool {
    matches!(
        tag,
        "projects.mutate"
            | "attachments.createUploadUrl"
            | "attachments.delete"
            | "assets.createUrl"
            | "projects.readFile"
            | "projects.searchEntries"
            | "filesystem.browse"
            | "filesystem.getMetadata"
            | "shell.openInEditor"
            | "vcs.refreshStatus"
            | "vcs.listRefs"
            | "vcs.createRef"
            | "vcs.switchRef"
            | "vcs.pull"
            | "vcs.init"
            | "review.getDiffPreview"
            | "review.getDiffFileContents"
            | "orchestration.getTurnDiff"
            | "orchestration.getFullThreadDiff"
            | "orchestration.searchThreads"
            | "orchestration.searchThread"
            | "server.discoverSourceControl"
            | "orchestration.getArchivedShellSnapshot"
            | "orchestration.getTurnItem"
            | "orchestration.getThreadProjection"
    )
}

pub async fn call(s: &State, tag: &str, p: Value) -> Result<Value, Value> {
    match tag {
        "projects.mutate" => crate::projects::mutate_resolved(s, &p).await.map_err(
            |message| serde_json::json!({"_tag":"ProjectMutationError","message":message}),
        ),
        "attachments.createUploadUrl" => assets::create_upload_url(s, &p),
        "attachments.delete" => assets::delete_attachment(s, &p),
        "assets.createUrl" => assets::create_url_async(s, &p).await,
        "projects.readFile" => Box::pin(files::read_file(p)).await,
        "projects.searchEntries" => Box::pin(files::search_entries(p)).await,
        "filesystem.browse" => Box::pin(files::browse(p)).await,
        "filesystem.getMetadata" => Box::pin(files::get_metadata(p)).await,
        "shell.openInEditor" => Box::pin(files::open_in_editor(p)).await,
        "vcs.refreshStatus" => Box::pin(vcs::refresh_status(p)).await,
        "vcs.listRefs" => Box::pin(vcs::list_refs(p)).await,
        "vcs.createRef" => Box::pin(vcs::create_ref(p)).await,
        "vcs.switchRef" => Box::pin(vcs::switch_ref(p)).await,
        "vcs.pull" => Box::pin(vcs::pull(p)).await,
        "vcs.init" => Box::pin(vcs::init(p)).await,
        "review.getDiffPreview" => Box::pin(review::diff_preview(s, p)).await,
        "review.getDiffFileContents" => Box::pin(review::file_contents(s, p)).await,
        "orchestration.getTurnDiff" => Box::pin(threadq::turn_diff(s, p)).await,
        "orchestration.getFullThreadDiff" => Box::pin(threadq::full_thread_diff(s, p)).await,
        "orchestration.searchThreads" => threadq::search_threads(s, &p),
        "orchestration.searchThread" => threadq::search_thread(s, &p),
        "server.discoverSourceControl" => crate::vcs::discover().await,
        "orchestration.getArchivedShellSnapshot" => threadq::archived_shell(s),
        "orchestration.getTurnItem" => threadq::turn_item(s, &p),
        "orchestration.getThreadProjection" => threadq::thread_projection(s, &p),
        _ => Err(
            serde_json::json!({"_tag":"RpcNotImplemented","message":format!("o3-server does not implement {tag}")}),
        ),
    }
}
