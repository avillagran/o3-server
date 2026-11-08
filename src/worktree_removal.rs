//! Explicit dependency boundary for destructive Git worktree and owned-branch removal.
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Worktree { path: PathBuf, force: bool },
    Branch { name: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub repository: PathBuf,
    pub operation: Operation,
}
impl Plan {
    pub fn worktree(repository: &Path, path: &Path, force: bool) -> Self {
        Self {
            repository: repository.to_owned(),
            operation: Operation::Worktree {
                path: path.to_owned(),
                force,
            },
        }
    }
    pub fn branch(repository: &Path, name: &str) -> Self {
        Self {
            repository: repository.to_owned(),
            operation: Operation::Branch { name: name.into() },
        }
    }
    pub fn argv(&self) -> Vec<String> {
        match &self.operation {
            Operation::Worktree { path, force } => {
                let mut args = vec!["worktree".into(), "remove".into()];
                if *force {
                    args.push("--force".into());
                }
                args.extend(["--".into(), path.to_string_lossy().into_owned()]);
                args
            }
            Operation::Branch { name } => {
                vec!["branch".into(), "-D".into(), "--".into(), name.clone()]
            }
        }
    }
}
pub trait Executor: Send + Sync {
    fn execute(&self, plan: &Plan) -> Result<(), String>;
}
/// An absent replacement preserves each caller's original real Git runner and timeout.
#[derive(Clone, Default)]
pub struct Registry(Arc<Mutex<Option<Arc<dyn Executor>>>>);
impl Registry {
    /// Configure an explicit executor before admitting operations; no environment bypass exists.
    pub fn set_executor(&self, executor: Arc<dyn Executor>) {
        *self.0.lock().unwrap() = Some(executor);
    }
    pub(crate) fn executor(&self) -> Option<Arc<dyn Executor>> {
        self.0.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_preserves_native_runner_and_plans_use_literal_argv() {
        crate::test_sandbox::assert_isolated();
        assert!(Registry::default().executor().is_none());
        let path = Path::new("/private/fixture/a quoted ' path");
        assert_eq!(
            Plan::worktree(Path::new("/private/repo"), path, true).argv(),
            vec![
                "worktree",
                "remove",
                "--force",
                "--",
                path.to_str().unwrap()
            ]
        );
        assert_eq!(
            Plan::worktree(Path::new("/private/repo"), path, false).argv(),
            vec!["worktree", "remove", "--", path.to_str().unwrap()]
        );
        assert_eq!(
            Plan::branch(Path::new("/private/repo"), "fixture/branch").argv(),
            vec!["branch", "-D", "--", "fixture/branch"]
        );
    }
}
