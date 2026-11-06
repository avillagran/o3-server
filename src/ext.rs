//! Dispatch for RPCs that need I/O or subprocesses. `rpc.rs` runs these on spawned tasks so a slow
//! `git pull` never stalls the rest of a socket.
use crate::{assets, files, review, store::State, threadq, vcs};
use serde_json::Value;
use std::sync::Arc;

pub const STREAM_ACTION: &str = "git.runStackedAction";

pub fn handles(tag: &str) -> bool {
    crate::p2::handles(tag)
        || crate::additions::handles(tag)
        || matches!(
            tag,
            "projects.mutate"
                | "sourceControl.cloneRepository"
                | "attachments.createUploadUrl"
                | "attachments.delete"
                | "assets.createUrl"
                | "projects.readFile"
                | "projects.searchEntries"
                | "projects.searchContents"
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
                | "review.getBranchCommits"
                | "orchestration.getTurnDiff"
                | "orchestration.getFullThreadDiff"
                | "orchestration.searchThreads"
                | "orchestration.searchThread"
                | "server.discoverSourceControl"
                | "orchestration.getArchivedShellSnapshot"
                | "orchestration.getTurnItem"
                | "orchestration.getThreadProjection"
                | "orchestration.getThreadTitles"
                | "orchestration.getThreadFamily"
                | "orchestration.getWorktreeThreadIds"
        )
}

pub async fn call(s: &Arc<State>, tag: &str, p: Value) -> Result<Value, Value> {
    match tag {
        "projects.mutate" => crate::projects::mutate_resolved(s, &p).await.map_err(
            |message| serde_json::json!({"_tag":"ProjectMutationError","message":message}),
        ),
        "sourceControl.cloneRepository" => crate::repositories::clone(s, &p, None).await,
        "attachments.createUploadUrl" => assets::create_upload_url(s, &p),
        "attachments.delete" => assets::delete_attachment(s, &p),
        "assets.createUrl" => assets::create_url_async(s, &p).await,
        "projects.readFile" => Box::pin(files::read_file(p)).await,
        "projects.searchEntries" => Box::pin(files::search_entries(p)).await,
        "projects.searchContents" => crate::content_search::search(p).await,
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
        "review.getBranchCommits" => Box::pin(review::branch_commits(s, p)).await,
        "orchestration.getTurnDiff" => Box::pin(threadq::turn_diff(s, p)).await,
        "orchestration.getFullThreadDiff" => Box::pin(threadq::full_thread_diff(s, p)).await,
        "orchestration.searchThreads" => threadq::search_threads(s, &p),
        "orchestration.searchThread" => threadq::search_thread(s, &p),
        "server.discoverSourceControl" => crate::vcs::discover().await,
        "orchestration.getArchivedShellSnapshot" => threadq::archived_shell(s),
        "orchestration.getTurnItem" => threadq::turn_item(s, &p),
        "orchestration.getThreadProjection" => threadq::thread_projection(s, &p),
        "orchestration.getThreadTitles" => threadq::thread_titles(s, &p),
        "orchestration.getThreadFamily" => threadq::thread_family(s, &p),
        "orchestration.getWorktreeThreadIds" => threadq::worktree_threads(s, &p),
        _ if crate::additions::handles(tag) => crate::additions::call(tag, &p),
        _ if crate::p2::handles(tag) => crate::p2::call(s, tag, p).await,
        _ => Err(
            serde_json::json!({"_tag":"RpcNotImplemented","message":format!("o3-server does not implement {tag}")}),
        ),
    }
}
