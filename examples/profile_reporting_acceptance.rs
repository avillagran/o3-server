//! Actual private HTTP/RPC/catalog owners for the QML -> client watch proof.
//! This retains all fixtures and never opens a provider, browser or host profile.
use o3_server::store::State;
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn save(path: &Path, value: &Value) -> io::Result<()> {
    let temporary = path.with_extension(format!(
        "pending-{}",
        o3_server::store::random_id().map_err(io::Error::other)?
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    std::fs::rename(temporary, path)
}

fn catalogs(states: &[(&str, Arc<State>)]) -> Value {
    Value::Object(
        states
            .iter()
            .map(|(name, state)| {
                let snapshot = state.streams.p2.preview.profile_catalog();
                let mut value = snapshot.status();
                value["recordingFrameRate"] = json!(snapshot.recording_frame_rate);
                ((*name).into(), value)
            })
            .collect(),
    )
}

async fn serve(listener: tokio::net::TcpListener, state: Arc<State>) -> io::Result<()> {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                let state = state.clone();
                connections.spawn(async move { o3_server::serve_io(socket, state).await });
            }
            finished = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = finished {
                    return Err(io::Error::other(error));
                }
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    assert!(Path::new("/run/.containerenv").is_file());
    let root = Path::new("/tmp/profile-report-chain");
    std::fs::create_dir(root)?;
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
    let mut states = Vec::new();
    let mut environments = Vec::new();
    let mut servers = Vec::new();
    for name in ["a", "b", "denied"] {
        let state = Arc::new(State::open(&root.join(name)).map_err(io::Error::other)?);
        state
            .configure_web_auth(Some("127.0.0.1"))
            .map_err(io::Error::other)?;
        let descriptor = o3_server::settings::descriptor(&state);
        if descriptor["capabilities"]["serverBrowser"] != true {
            return Err(io::Error::other(
                "An actual browser executable is required for discovery",
            ));
        }
        let scopes = if name == "denied" {
            vec!["orchestration:read".to_owned()]
        } else {
            vec![
                "orchestration:read".to_owned(),
                "preview:operate".to_owned(),
            ]
        };
        let token = state
            .issue_session("private-profile-chain", &scopes, 120_000, "private-fixture")
            .map_err(io::Error::other)?;
        let token_file = root.join(format!("{name}.token"));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&token_file)?;
        file.write_all(token.as_bytes())?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        environments.push(json!({"id":name,"base_url":format!("http://{address}"),
            "token_file":token_file,"write_token_file":token_file}));
        states.push((name, state.clone()));
        servers.push(tokio::spawn(serve(listener, state)));
    }
    let mut previous = catalogs(&states);
    save(Path::new("/evidence/catalog.json"), &previous)?;
    save(
        &root.join("environments.json"),
        &json!({"environments":environments}),
    )?;
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let control = std::thread::spawn(move || {
        let line = io::stdin().lock().lines().next().transpose();
        let valid = matches!(line, Ok(Some(ref text))
            if serde_json::from_str::<Value>(text).ok() == Some(json!({"op":"stop"})));
        let _ = stop.send(valid);
    });
    let started = Instant::now();
    let mut timeline = vec![json!({"elapsedMs":0,"catalogs":previous})];
    let mut tick = tokio::time::interval(Duration::from_millis(40));
    let clean_stop = loop {
        tokio::select! {
            stop = &mut stopped => break stop.unwrap_or(false),
            _ = tick.tick() => {
                let current = catalogs(&states);
                if current != previous {
                    save(Path::new("/evidence/catalog.json"), &current)?;
                    timeline.push(json!({"elapsedMs":started.elapsed().as_millis(),"catalogs":current}));
                    previous = current;
                }
                if servers.iter().any(tokio::task::JoinHandle::is_finished)
                    || started.elapsed() > Duration::from_secs(45) {
                    break false;
                }
            }
        }
    };
    for (_, state) in &states {
        state.begin_shutdown();
    }
    for server in servers {
        server.abort();
        let _ = server.await;
    }
    if control.is_finished() {
        control
            .join()
            .map_err(|_| io::Error::other("Fixture control failed"))?;
    }
    save(
        Path::new("/evidence/server-result.json"),
        &json!({"cleanStop":clean_stop,
        "listeners":3,"timeline":timeline,"final":catalogs(&states),
        "scope":"Actual State, HTTP serve_io, bearer authentication, framed RPC and connection-owned catalog. No standalone startup, provider, browser engine, synthetic RPC peer or profile-report injection."}),
    )?;
    if !clean_stop {
        return Err(io::Error::other(
            "Private fixture stop was not acknowledged",
        ));
    }
    Ok(())
}
