//! Host-owned loopback forwards. Desired state survives host and desktop exit.
use omabridge::Store;
use serde_json::{json, Value};
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};
use tokio::{io::AsyncReadExt, task::JoinHandle};

fn desired(store: &Store) -> anyhow::Result<Value> {
    let path = store.root().join("forwards.json");
    if !path.exists() {
        return Ok(json!({}));
    }
    let doc: Value = omabridge::read_document(&path)?;
    anyhow::ensure!(
        doc.as_object().is_some_and(|d| d.len() <= 64),
        "invalid forward registry"
    );
    Ok(doc)
}
pub fn execute(store: &Store, request: &Value) -> anyhow::Result<Value> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(store.root().join("forwards.lock"))?;
    anyhow::ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0,
        "forward lock failed"
    );
    let mut doc = desired(store)?;
    let peer = request["peer"].as_str().unwrap_or("");
    match request["action"].as_str() {
        Some("read") => {
            let running =
                std::os::unix::net::UnixStream::connect(store.root().join("control.sock")).is_ok();
            let runtime = if running {
                omabridge::read_document::<Value>(&store.root().join("forwards-status.json"))
                    .unwrap_or(json!({}))
            } else {
                json!({})
            };
            return Ok(json!({"desired":doc,"running":running,"runtime":runtime}));
        }
        Some("save") => {
            anyhow::ensure!(
                doc.as_object()
                    .is_some_and(|d| d.len() < 64 || d.contains_key(peer)),
                "forward limit"
            );
            let peer = omabridge::find_peer(store, peer)
                .map_err(anyhow::Error::msg)?
                .id;
            let listen: SocketAddr = request["listen"].as_str().unwrap_or("").parse()?;
            anyhow::ensure!(
                listen.ip().is_loopback() && listen.port() != 0,
                "fixed loopback address required"
            );
            anyhow::ensure!(request["service"] == "t3", "unsupported service");
            doc[&peer] = json!({"listen":listen.to_string(),"service":"t3"});
        }
        Some("remove") => {
            if let Some(d) = doc.as_object_mut() {
                d.remove(peer);
            }
        }
        _ => anyhow::bail!("invalid action"),
    }
    omabridge::write_document(&store.root().join("forwards.json"), &doc, true)?;
    Ok(doc)
}
struct Forward {
    spec: Value,
    task: JoinHandle<()>,
}
impl Drop for Forward {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub async fn run(store: Store) {
    let mut active: BTreeMap<String, Forward> = BTreeMap::new();
    let mut last_status = None;
    loop {
        if let Ok(doc) = desired(&store) {
            active.retain(|peer, f| doc.get(peer) == Some(&f.spec) && !f.task.is_finished());
            for (peer, spec) in doc.as_object().into_iter().flatten() {
                if active.contains_key(peer) {
                    continue;
                }
                let Some(listen) = spec["listen"]
                    .as_str()
                    .and_then(|s| s.parse::<SocketAddr>().ok())
                else {
                    continue;
                };
                if !listen.ip().is_loopback() || listen.port() == 0 || spec["service"] != "t3" {
                    continue;
                }
                let result = tokio::time::timeout(
                    Duration::from_secs(2),
                    omabridge::forward_through_server(&store, peer, "t3", listen, None),
                )
                .await;
                if let Ok(Some(Ok((mut socket, _)))) = result {
                    let task = tokio::spawn(async move {
                        let mut b = [0; 64];
                        while matches!(socket.read(&mut b).await, Ok(n) if n > 0) {}
                    });
                    active.insert(
                        peer.clone(),
                        Forward {
                            spec: spec.clone(),
                            task,
                        },
                    );
                }
            }
            let status: BTreeMap<_, _> = doc
                .as_object()
                .into_iter()
                .flatten()
                .map(|(peer, _)| {
                    (
                        peer,
                        if active.contains_key(peer) {
                            "listening"
                        } else {
                            "retrying"
                        },
                    )
                })
                .collect();
            let value = serde_json::to_value(status).unwrap();
            if last_status.as_ref() != Some(&value)
                && omabridge::write_document(
                    &store.root().join("forwards-status.json"),
                    &value,
                    true,
                )
                .is_ok()
            {
                last_status = Some(value);
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
