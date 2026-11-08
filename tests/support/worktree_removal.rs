#[path = "sandbox.rs"]
mod test_sandbox;
// Explicit private-fixture simulation: retain trees/metadata by rename, never execute removal.
pub fn assert_isolated() {
    test_sandbox::assert_isolated();
}
use o3_server::worktree_removal::{Executor, Operation, Plan};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Clone, Debug)]
pub struct Retained {
    pub original: PathBuf,
    pub path: PathBuf,
    pub kind: &'static str,
}
pub struct RetainingExecutor {
    roots: Vec<PathBuf>,
    quarantine: PathBuf,
    pub plans: Mutex<Vec<Plan>>,
    pub retained: Mutex<Vec<Retained>>,
}
impl RetainingExecutor {
    pub fn install(state: &o3_server::store::State) -> Arc<Self> {
        Self::install_with_roots(state, vec![state.dir.clone()])
    }
    pub fn install_with_roots(state: &o3_server::store::State, roots: Vec<PathBuf>) -> Arc<Self> {
        assert_isolated();
        let value = Arc::new(Self {
            roots,
            quarantine: state.dir.join("retained-worktree-simulation"),
            plans: Mutex::new(Vec::new()),
            retained: Mutex::new(Vec::new()),
        });
        state.worktree_removal.set_executor(value.clone());
        value
    }
    fn checked(&self, path: &Path) -> Result<(), String> {
        if !path.is_absolute() || !self.roots.iter().any(|root| path.starts_with(root)) {
            return Err("Simulation path is outside its explicit private fixture".into());
        }
        let mut current = PathBuf::new();
        for part in path.components() {
            match part {
                std::path::Component::RootDir | std::path::Component::Normal(_) => {
                    current.push(part)
                }
                _ => return Err("Invalid simulation path".into()),
            }
            if std::fs::symlink_metadata(&current)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            {
                return Err("Simulation cannot follow links".into());
            }
        }
        Ok(())
    }
    fn git(&self, root: &Path, args: &[&str]) -> Result<String, String> {
        self.checked(root)?;
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err("Read-only fixture Git inspection failed".into());
        }
        Ok(String::from_utf8(out.stdout)
            .map_err(|e| e.to_string())?
            .trim()
            .to_owned())
    }
    fn retain(&self, path: &Path, kind: &'static str) -> Result<(), String> {
        self.checked(path)?;
        if self.roots.iter().any(|root| path == root) {
            return Err("Cannot retain an entire fixture root".into());
        }
        o3_server::cleanup::ensure_directory(&self.quarantine).map_err(|e| e.to_string())?;
        let target = self
            .quarantine
            .join(format!("{}-{kind}", o3_server::store::random_id()?));
        o3_server::cleanup::publish_directory(path, &target).map_err(|e| e.to_string())?;
        self.retained.lock().unwrap().push(Retained {
            original: path.to_owned(),
            path: target,
            kind,
        });
        Ok(())
    }
    pub fn assert_retained_worktree(&self, original: &Path) {
        let retained = self.retained.lock().unwrap();
        let record = retained
            .iter()
            .find(|record| record.original == original && record.kind == "checkout")
            .expect("Recorded cleanup must retain the exact fixture worktree");
        assert!(record.path.is_dir());
        assert!(record.path.join(".git").is_file());
        assert!(
            retained
                .iter()
                .any(|registration| registration.kind == "registration"
                    && std::fs::read_to_string(registration.path.join("gitdir"))
                        .is_ok_and(|value| Path::new(value.trim()) == original.join(".git"))),
            "The exact Git registration must also be retained"
        );
        assert!(self
            .plans
            .lock()
            .unwrap()
            .iter()
            .any(|plan| matches!(&plan.operation,Operation::Worktree{path,..} if path==original)));
    }
}
impl Executor for RetainingExecutor {
    fn execute(&self, plan: &Plan) -> Result<(), String> {
        self.plans.lock().unwrap().push(plan.clone());
        self.checked(&plan.repository)?;
        let common = PathBuf::from(self.git(
            &plan.repository,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?);
        self.checked(&common)?;
        match &plan.operation {
            Operation::Worktree { path, force: _ } => {
                self.checked(path)?;
                let registration =
                    PathBuf::from(self.git(path, &["rev-parse", "--absolute-git-dir"])?);
                self.checked(&registration)?;
                if registration.parent() != Some(common.join("worktrees").as_path()) {
                    return Err(
                        "Fixture worktree registration is not owned by the supplied repository"
                            .into(),
                    );
                }
                let pointer = registration.join("gitdir");
                self.checked(&pointer)?;
                let linked = std::fs::read_to_string(pointer).map_err(|e| e.to_string())?;
                if Path::new(linked.trim()) != path.join(".git") {
                    return Err("Fixture registration points elsewhere".into());
                }
                if registration.join("locked").symlink_metadata().is_ok() {
                    return Err("Locked worktree requires more than one force flag".into());
                }
                self.retain(path, "checkout")?;
                self.retain(&registration, "registration")?;
            }
            Operation::Branch { name } => {
                self.git(&plan.repository, &["check-ref-format", "--branch", name])?;
                let reference = common.join("refs/heads").join(name);
                self.checked(&reference)?;
                if !reference.is_file() {
                    return Err("Fixture branch is not an owned loose ref".into());
                }
                let reflog = common.join("logs/refs/heads").join(name);
                if reflog.symlink_metadata().is_ok() {
                    self.checked(&reflog)?;
                    if !reflog.is_file() {
                        return Err("Invalid fixture branch reflog".into());
                    }
                }
                self.retain(&reference, "ref")?;
                if reflog.symlink_metadata().is_ok() {
                    self.retain(&reflog, "reflog")?;
                }
            }
        }
        Ok(())
    }
}
