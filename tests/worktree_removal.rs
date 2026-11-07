//! Actual workspace preparation with explicitly retained rollback effects.
#[path = "support/worktree_removal.rs"]
mod retained_worktrees;
mod support;
use serde_json::json;
use std::path::Path;
fn git(root: &Path, args: &[&str]) {
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .status()
        .unwrap()
        .success());
}
#[test]
fn prepared_rollback_records_and_retains_exact_checkout_registration_and_branch() {
    retained_worktrees::assert_isolated();
    let server = support::start(|_| {});
    let state = &server.state;
    let removal = retained_worktrees::RetainingExecutor::install(state);
    let root = state.dir.join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "--initial-branch=main"]);
    std::fs::write(root.join("kept"), "fixture contents").unwrap();
    git(&root, &["add", "kept"]);
    git(&root, &["commit", "-m", "fixture"]);
    o3_server::events::mutate_project(state,&json!({"type":"project.create","commandId":"p","projectId":"p","title":"Fixture","workspaceRoot":root})).unwrap();
    let request = json!({"projectId":"p","workspaceStrategy":{"type":"worktree","baseRef":"HEAD","branch":"fixture-owned"}});
    let mut prepared = o3_server::workspace::prepare(state, &request, "private-thread").unwrap();
    let path = std::path::PathBuf::from(prepared.path.as_str().unwrap());
    assert!(prepared.is_owned());
    prepared.rollback().unwrap();
    removal.assert_retained_worktree(&path);
    let records = removal.retained.lock().unwrap();
    let checkout = records
        .iter()
        .find(|record| record.original == path && record.kind == "checkout")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(checkout.path.join("kept")).unwrap(),
        "fixture contents"
    );
    assert!(records.iter().any(|record| record.kind == "ref"));
    drop(records);
    let plans = removal.plans.lock().unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].repository, root);
    assert_eq!(
        plans[0].argv(),
        vec![
            "worktree",
            "remove",
            "--force",
            "--",
            path.to_str().unwrap()
        ]
    );
    assert_eq!(plans[1].argv(), vec!["branch", "-D", "--", "fixture-owned"]);
    drop(plans);
    prepared.rollback().unwrap();
    assert_eq!(
        removal.plans.lock().unwrap().len(),
        2,
        "Ownership is consumed exactly once"
    );
    let mut retry = o3_server::workspace::prepare(state, &request, "private-thread").unwrap();
    assert!(retry.is_owned());
    assert!(Path::new(retry.path.as_str().unwrap())
        .join("kept")
        .is_file());
    retry.commit();
    retry.rollback().unwrap();
    assert_eq!(removal.plans.lock().unwrap().len(), 2);
}

#[test]
fn simulator_refuses_locked_or_linked_registration_without_moving_fixture_data() {
    use o3_server::worktree_removal::{Executor, Plan};
    retained_worktrees::assert_isolated();
    let server = support::start(|_| {});
    let state = &server.state;
    let removal = retained_worktrees::RetainingExecutor::install(state);
    let root = state.dir.join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["commit", "--allow-empty", "-m", "fixture"]);
    let work = state.dir.join("owned-worktree");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "fixture-guard",
            work.to_str().unwrap(),
        ],
    );
    git(&root, &["worktree", "lock", work.to_str().unwrap()]);
    let plan = Plan::worktree(&root, &work, true);
    assert!(removal.execute(&plan).is_err());
    assert!(work.join(".git").is_file());
    assert!(removal.retained.lock().unwrap().is_empty());
    git(&root, &["worktree", "unlock", work.to_str().unwrap()]);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&work)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let registration = std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    assert!(registration.starts_with(root.join(".git/worktrees")));
    let pointer = registration.join("gitdir");
    let saved = state.dir.join("saved-fixture-gitdir");
    std::fs::rename(&pointer, &saved).unwrap();
    let sibling = state.dir.join("private-target");
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(sibling.join("keep"), "untouched").unwrap();
    std::os::unix::fs::symlink(&sibling, &pointer).unwrap();
    assert!(removal.execute(&plan).is_err());
    assert!(work.join(".git").is_file());
    assert!(removal.retained.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(sibling.join("keep")).unwrap(),
        "untouched"
    );
    std::fs::rename(&pointer, state.dir.join("retained-fixture-link")).unwrap();
    std::fs::rename(&saved, &pointer).unwrap();
    removal.execute(&plan).unwrap();
    removal.assert_retained_worktree(&work);
    assert!(registration.symlink_metadata().is_err());
    assert_eq!(
        std::fs::read_to_string(sibling.join("keep")).unwrap(),
        "untouched"
    );
}
