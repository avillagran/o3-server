//! Setup ownership survives session changes and server death. The unique inherited
//! marker also catches double-forked descendants after they have been reparented.
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

fn stat(pid: i32) -> Option<(i32, i32, i32, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = text.rsplit_once(") ")?.1.split_whitespace().collect();
    Some((
        fields.get(1)?.parse().ok()?,
        fields.get(2)?.parse().ok()?,
        fields.get(3)?.parse().ok()?,
        fields.get(19)?.parse().ok()?,
    ))
}
fn processes(token: &str) -> Option<HashMap<i32, u64>> {
    let marker = format!("O3_SETUP_OWNER={token}");
    let mut owned = HashSet::new();
    let mut tree = HashMap::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Some(info) = stat(pid) else { continue };
        if let Ok(env) = std::fs::read(entry.path().join("environ")) {
            if env.split(|b| *b == 0).any(|v| v == marker.as_bytes()) {
                owned.insert(pid);
            }
        }
        tree.insert(pid, info);
    }
    loop {
        let before = owned.len();
        for (pid, (parent, group, session, _)) in &tree {
            // Never adopt the server's shared session: only sessions/groups led
            // by an already owned setup process can extend ownership.
            if owned.contains(parent) || owned.contains(group) || owned.contains(session) {
                owned.insert(*pid);
            }
        }
        if owned.len() == before {
            return Some(
                owned
                    .into_iter()
                    .filter_map(|pid| tree.get(&pid).map(|v| (pid, v.3)))
                    .collect(),
            );
        }
    }
}

pub fn stop(token: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let Some(owned) = processes(token) else {
            return false;
        };
        if owned.is_empty() {
            return true;
        }
        for (pid, started) in owned {
            // Recheck process identity before signaling; a reused PID is not owned.
            if stat(pid).is_some_and(|v| v.3 == started) {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
