//! Opt-in acceptance on a copy of a real profile: O3 writes its history into the copy's
//! `statev2.sqlite`, then the real T3 starts on the same copy and must read every thread.
//!
//! `O3_SHARED_PROFILE` names the copy (never a live profile: the run writes into it) and
//! `T3_REFERENCE_BIN` the T3 runtime. Both servers run with a private HOME and no credentials.
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use t3_protocol::{Connection as Rpc, Credential};

struct Server {
    child: Child,
    base: String,
}
impl Server {
    /// SIGTERM to the process group, then wait so the exporter's final flush completes.
    fn stop(mut self, grace: Duration) -> std::process::ExitStatus {
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        let deadline = Instant::now() + grace;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                }
                return self.child.wait().unwrap();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn command(bin: &Path, profile: &Path, scratch: &Path) -> Command {
    let mut c = Command::new(bin);
    let parent = std::process::id();
    unsafe {
        c.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent as i32 {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    let path = std::env::var("ACCEPTANCE_PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
    c.env_clear()
        .env("PATH", path)
        .env("HOME", scratch.join("home"))
        .env("T3CODE_HOME", profile)
        .env("TMPDIR", scratch.join("tmp"))
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("XDG_CACHE_HOME", scratch.join("cache"))
        .env("XDG_DATA_HOME", scratch.join("data"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("XDG_RUNTIME_DIR", scratch.join("run"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .current_dir(scratch)
        .stdin(Stdio::null());
    c
}

fn serve(bin: &Path, profile: &Path, scratch: &Path, name: &str, extra: &[&str]) -> Server {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let log = std::fs::File::create(scratch.join(format!("{name}.log"))).unwrap();
    let child = command(bin, profile, scratch)
        .args(["serve", "--host", "127.0.0.1", "--port", &port.to_string()])
        .args(extra)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .process_group(0)
        .spawn()
        .unwrap();
    let mut server = Server {
        child,
        base: format!("http://127.0.0.1:{port}"),
    };
    let deadline = Instant::now() + Duration::from_secs(600);
    while t3_protocol::fetch_descriptor(&server.base).is_err() {
        assert!(
            server.child.try_wait().unwrap().is_none(),
            "{name} exited; see {}",
            scratch.join(format!("{name}.log")).display()
        );
        assert!(Instant::now() < deadline, "{name} did not listen");
        std::thread::sleep(Duration::from_millis(250));
    }
    server
}

fn read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn one<T: rusqlite::types::FromSql>(db: &Connection, sql: &str) -> T {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
#[ignore = "needs O3_SHARED_PROFILE (a copy) and T3_REFERENCE_BIN"]
fn real_t3_reads_what_o3_wrote_into_a_copied_profile() {
    let profile = PathBuf::from(std::env::var_os("O3_SHARED_PROFILE").expect("O3_SHARED_PROFILE"));
    let t3_bin = PathBuf::from(std::env::var_os("T3_REFERENCE_BIN").expect("T3_REFERENCE_BIN"));
    let userdata = profile.join("userdata");
    assert!(userdata.join("statev2.sqlite").is_file() && userdata.join("o3.sqlite").is_file());
    let scratch = profile.join("acceptance");
    for dir in ["home", "tmp", "config", "cache", "data", "state", "run"] {
        std::fs::create_dir_all(scratch.join(dir)).unwrap();
    }
    // The copy carries the live server's runtime file; neither server may treat it as owned.
    let _ = std::fs::remove_file(userdata.join("server-runtime.json"));

    // 1. O3 on the copy adopts and exports, then stops with a final flush.
    let started = Instant::now();
    let o3 = serve(
        Path::new(env!("CARGO_BIN_EXE_o3-server")),
        &profile,
        &scratch,
        "o3",
        &["--no-bridge"],
    );
    let marks = read_only(&userdata.join("o3.sqlite"));
    let deadline = Instant::now() + Duration::from_secs(3600);
    loop {
        let left: i64 = one(&marks, "SELECT count(*) FROM t3_dirty");
        if left == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "{left} records still marked");
        std::thread::sleep(Duration::from_secs(2));
    }
    let status = o3.stop(Duration::from_secs(120));
    eprintln!("o3 exported in {:?}, exit {status}", started.elapsed());
    let rejects: Vec<(String, String, i64)> = {
        let mut q = marks
            .prepare(
                "SELECT kind,error,count(*) FROM t3_rejects GROUP BY 1,2 ORDER BY 3 DESC LIMIT 20",
            )
            .unwrap();
        let rows = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    };
    eprintln!("rejects: {rejects:?}");

    // 2. The invariants T3 checks before trusting its projections.
    let t3db = read_only(&userdata.join("statev2.sqlite"));
    let latest: i64 = one(&t3db, "SELECT coalesce(max(sequence),0) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread'");
    let last: i64 = one(&t3db, "SELECT last_sequence FROM orchestration_v2_projection_metadata WHERE projection_name='thread-projections'");
    assert_eq!(last, latest, "projection high-water mark");
    let unanchored: i64 = one(&t3db, "SELECT count(*) FROM orchestration_v2_projection_threads t WHERE NOT EXISTS (SELECT 1 FROM orchestration_events e WHERE e.application_event_version=2 AND e.aggregate_kind='thread' AND e.event_type='thread.created' AND e.stream_id=t.thread_id)");
    assert_eq!(unanchored, 0, "threads without thread.created");
    let threads: Vec<(String, String)> = {
        let mut q = t3db
            .prepare("SELECT thread_id,title FROM orchestration_v2_projection_threads WHERE deleted_at IS NULL ORDER BY updated_at DESC")
            .unwrap();
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    };
    drop(t3db);
    drop(marks);

    // 3. Real T3 on the same copy reads every live thread through its own decoders.
    let issued = command(&t3_bin, &profile, &scratch)
        .args([
            "auth",
            "session",
            "issue",
            "--subject",
            "acceptance",
            "--ttl",
            "1h",
            "--token-only",
        ])
        .output()
        .unwrap();
    assert!(
        issued.status.success(),
        "{}",
        String::from_utf8_lossy(&issued.stderr)
    );
    let token = String::from_utf8(issued.stdout).unwrap().trim().to_owned();
    let t3 = serve(&t3_bin, &profile, &scratch, "t3", &[]);
    let rpc = Rpc::new(&t3.base, None, Credential::bearer(&token).unwrap()).unwrap();
    let mut failures = Vec::new();
    for (id, title) in &threads {
        match rpc.rpc("orchestration.getThreadProjection", json!({"threadId": id})) {
            Ok(projection) => {
                let thread: &Value = &projection["thread"];
                if thread["title"] != json!(title) {
                    failures.push(format!("{id}: title {} != {title}", thread["title"]));
                }
            }
            Err(e) => failures.push(format!("{id}: {e}")),
        }
    }
    let status = t3.stop(Duration::from_secs(30));
    eprintln!(
        "t3 read {} threads, {} failed, exit {status}",
        threads.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
}
