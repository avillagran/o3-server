//! Record actual owned browser cleanup plans without deleting fixture trees.
#[derive(Default)]
pub struct Recording(pub std::sync::Mutex<Vec<o3_server::cleanup::RemovalPlan>>);
impl o3_server::cleanup::Removal for Recording {
    fn execute(&self, plan: &o3_server::cleanup::RemovalPlan) -> std::io::Result<()> {
        let root = &plan
            .operations()
            .last()
            .expect("Profile plan has a root")
            .path;
        let pid: i32 = std::fs::read_to_string(root.join("fixture.pid"))?
            .trim()
            .parse()
            .map_err(std::io::Error::other)?;
        // SAFETY: signal zero only checks this private fixture child's existence.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "Browser must be reaped before cleanup planning is executed"
        );
        self.0.lock().unwrap().push(plan.clone());
        Ok(())
    }
}
