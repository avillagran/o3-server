//! Embedded OmaBridge uses this process's Tokio runtime and auth database.
use crate::store::State;
use omabridge::t3_pair::{PairingIssuer, T3Pairing};
use omabridge::{LocalService, NetOptions, Node, NodeConfig, PeerInfo, Store, Target};
use std::{collections::BTreeMap, path::Path, sync::Arc};

type PairFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<T3Pairing>> + Send + 'a>>;
struct Service(Arc<State>);
impl LocalService for Service {
    fn serve(
        self: Arc<Self>,
        io: tokio::io::DuplexStream,
        _peer: PeerInfo,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            let _ = crate::serve_io(io, self.0.clone()).await;
        })
    }
}
pub struct Issuer(pub Arc<State>);
impl Issuer {
    fn labelled(&self, label: String) -> anyhow::Result<T3Pairing> {
        let receipt = self
            .0
            .issue_pairing(Some(&label), 120_000)
            .map_err(anyhow::Error::msg)?;
        let pairing = T3Pairing {
            environment_id: self.0.environment_id.clone(),
            credential: receipt["credential"].as_str().unwrap().into(),
            label,
        };
        pairing.validate()?;
        Ok(pairing)
    }
}
impl PairingIssuer for Issuer {
    fn issue<'a>(&'a self, _target: Option<std::net::SocketAddr>, peer: &'a str) -> PairFuture<'a> {
        Box::pin(async move { self.labelled(format!("omabridge:{peer}")) })
    }
    fn issue_delegated<'a>(
        &'a self,
        _target: Option<std::net::SocketAddr>,
        bridge: &'a str,
        client: &'a str,
        environment: &'a str,
    ) -> PairFuture<'a> {
        Box::pin(async move {
            anyhow::ensure!(self.0.environment_id == environment, "not this environment");
            self.labelled(omabridge::t3_pair::delegated_label(bridge, client))
        })
    }
}
/// State must be explicit and isolated from any independently running bridge.
pub async fn start(path: &Path, state: Arc<State>, options: NetOptions) -> anyhow::Result<Node> {
    start_hosted(path, state, options, None, false).await
}
/// Gateway settings are explicit JSON; absent settings allocate no listener or poller.
/// `bind`, `certificates` (host -> [cert, key]), and `gateways` are required.
pub async fn start_hosted(
    path: &Path,
    state: Arc<State>,
    options: NetOptions,
    gateway: Option<&Path>,
    relay_host: bool,
) -> anyhow::Result<Node> {
    anyhow::ensure!(
        state.managed_policy.allows_remote(),
        "{}",
        crate::managed_clients::refusal(
            &state.managed_policy,
            "remote clients",
            Some("so OmaBridge cannot expose this environment")
        )
    );
    let store = Store::open(path, "o3-server")?;
    omabridge::write_document(
        &path.join("host.json"),
        &serde_json::json!({"executable": std::env::current_exe()?}),
        true,
    )?;
    let reservation = store.reserve_runtime()?;
    let backend = Arc::new(
        omabridge::manage::EmbeddedBackend::new(store.clone(), vec!["t3".into()])
            .map_err(anyhow::Error::msg)?,
    );
    let mut config = NodeConfig::embedded();
    config.backend = Some(backend.clone());
    config.relay_host = relay_host;
    let service: Arc<dyn LocalService> = Arc::new(Service(state.clone()));
    if let Some(path) = gateway {
        let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let bind = doc["bind"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("gateway bind required"))?
            .parse()?;
        let certificates = serde_json::from_value(doc["certificates"].clone())?;
        let mut gateways: Vec<omabridge::gateway::Gateway> =
            serde_json::from_value(doc["gateways"].clone())?;
        for gateway in &mut gateways {
            if gateway.peer == "self" {
                gateway.peer = store.id()?;
            }
        }
        omabridge::gateway::save(store.root(), &gateways)?;
        config.gateways = true;
        config.gateway_runtime = Some(omabridge::gateway::GatewayRuntime {
            bind,
            certificates,
            local: BTreeMap::from([("t3".into(), service.clone())]),
            backend,
        });
    }
    let mut node = Node::start_configured(
        store.clone(),
        BTreeMap::from([("t3".into(), Target::Local(service))]),
        options,
        reservation,
        None,
        None,
        Some(Arc::new(Issuer(state.clone()))),
        config,
    )
    .await?;
    store.publish_endpoint(&node.addr(), &["t3".into()])?;
    node.attach_task(tokio::spawn(crate::bridge_forwards::run(store.clone())));
    node.attach_task(tokio::spawn(async move {
        loop {
            let mut endpoints = Vec::new();
            if let Ok(routes) = store.service_routes(omabridge::unix_time().unwrap_or(0)) {
                for route in routes.into_iter().filter(|r| r.service == "t3").take(8) {
                    let url = route.url.clone();
                    let environment = state.environment_id.clone();
                    let verified = tokio::task::spawn_blocking(move || {
                        t3_protocol::verify_environment(
                            &url,
                            &environment,
                            &t3_protocol::ConnectionOptions::default()
                                .with_request_timeout(std::time::Duration::from_secs(2)),
                        )
                    })
                    .await;
                    if matches!(verified, Ok(Ok(_))) {
                        endpoints
                            .push(serde_json::json!({"kind":"omabridge", "httpBaseUrl":route.url}));
                    }
                }
            }
            {
                let mut current = state.bridge_endpoints.lock().unwrap();
                if *current != endpoints {
                    *current = endpoints;
                    state.notify();
                }
            }
            if let Ok(publisher) = omabridge::catalog_publisher::Publisher::new() {
                let _ = publisher.publish_auto(&store).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    }));
    Ok(node)
}

/// Desktop management facade; never discovers or starts a standalone service.
pub fn cli(args: &[String]) -> std::io::Result<()> {
    use std::io::Read;
    let dir = args
        .windows(2)
        .find(|w| w[0] == "--state-dir")
        .map(|w| Path::new(&w[1]))
        .ok_or_else(|| std::io::Error::other("state directory required"))?;
    let store = Store::open(dir, "o3-server").map_err(std::io::Error::other)?;
    let doc: serde_json::Value =
        omabridge::read_document(&dir.join("services.json")).map_err(std::io::Error::other)?;
    if doc["backend"] != "embedded" {
        return Err(std::io::Error::other("embedded state required"));
    }
    if args.iter().any(|a| a == "peers") {
        println!(
            "{}",
            serde_json::json!({"peers":store.peers().map_err(std::io::Error::other)?})
        );
        return Ok(());
    }
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "forward" | "t3-pair"))
    {
        let opt = |key: &str| args.windows(2).find(|w| w[0] == key).map(|w| w[1].clone());
        let peer = opt("--peer").ok_or_else(|| std::io::Error::other("peer required"))?;
        let peer = omabridge::find_peer(&store, &peer)
            .map_err(std::io::Error::other)?
            .id;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .enable_all()
            .build()?;
        return runtime.block_on(async {
            if args.iter().any(|a| a == "t3-pair") {
                let pairing = omabridge::t3_pair_through_server(&store, &peer).await
                    .ok_or_else(|| std::io::Error::other("embedded control socket unavailable"))?.map_err(std::io::Error::other)?;
                println!("{}", serde_json::to_string(&pairing)?);
                return Ok(());
            }
            let service = opt("--service").unwrap_or_else(|| "t3".into());
            let listen = opt("--listen").unwrap_or_else(|| "127.0.0.1:0".into()).parse().map_err(std::io::Error::other)?;
            let (mut socket, local) = omabridge::forward_through_server(&store, &peer, &service, listen, None).await
                .ok_or_else(|| std::io::Error::other("embedded control socket unavailable"))?.map_err(std::io::Error::other)?;
            println!("{}", serde_json::json!({"event":"forwarding","listen":local.to_string(),"peer":peer,"service":service}));
            use tokio::io::AsyncReadExt;
            let mut sink = [0;64];
            while socket.read(&mut sink).await? > 0 {}
            Err(std::io::Error::other("embedded server stopped"))
        });
    }
    let mut raw = Vec::new();
    std::io::stdin().take(65537).read_to_end(&mut raw)?;
    let request = if raw.len() <= 65536 {
        serde_json::from_slice(&raw).ok()
    } else {
        None
    };
    if args.iter().any(|a| a == "forwards") {
        let value = crate::bridge_forwards::execute(&store, &request.unwrap_or_default())
            .map_err(std::io::Error::other)?;
        println!("{value}");
        return Ok(());
    }
    let backend = omabridge::manage::EmbeddedBackend::new(store.clone(), vec!["t3".into()])
        .map_err(std::io::Error::other)?;
    let manager = omabridge::manage::Manager::embedded(&store, Arc::new(backend));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .enable_all()
        .build()?;
    let result = match request {
        Some(v) => runtime.block_on(manager.execute(&v)),
        None => Err("invalid_command"),
    };
    match result {
        Ok(v) => println!("{v}"),
        Err(code) => {
            println!("{}", serde_json::json!({"error":code}));
            std::process::exit(1);
        }
    }
    Ok(())
}

#[cfg(test)]
mod managed_client_tests {
    use super::*;
    fn state(limits: serde_json::Value) -> Arc<State> {
        assert!(Path::new("/run/.containerenv").is_file());
        let dir = std::env::temp_dir().join(format!(
            "managed-bridge-{}",
            crate::store::random_id().unwrap()
        ));
        let mut state = State::open(&dir).unwrap();
        state.managed_policy = crate::managed_policy::Policy::parse(
            Path::new("/fixture/policy.json"),
            &serde_json::json!({"schemaVersion":1,"organization":"Fixture Corp","limits":limits})
                .to_string(),
        )
        .unwrap();
        Arc::new(state)
    }
    #[tokio::test]
    async fn policy_refuses_bridge_publication_and_credentials_before_state_effects() {
        let state = state(serde_json::json!({"clients":["local-browser"]}));
        let path = state.dir.join("bridge-not-created");
        let error =
            match start_hosted(&path, state.clone(), NetOptions::loopback(), None, false).await {
                Ok(_) => panic!("Disallowed bridge started"),
                Err(error) => error,
            };
        assert!(error.to_string().contains("Fixture Corp"));
        assert!(!path.exists());
        assert!(Issuer(state.clone())
            .issue(None, "fixture-peer")
            .await
            .is_err());
        let count: i64 = state
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM auth_pairing_links", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
    #[tokio::test]
    async fn connect_disabled_keeps_independent_bridge_pairing_available() {
        let state = state(serde_json::json!({"connect":false}));
        let pairing = Issuer(state.clone())
            .issue(None, "fixture-peer")
            .await
            .unwrap();
        let exchanged = state
            .exchange_pairing(&pairing.credential, &serde_json::json!({}))
            .unwrap()
            .unwrap();
        assert_eq!(
            state
                .session(exchanged["access_token"].as_str().unwrap())
                .unwrap()["sub"],
            crate::managed_clients::LINK_SUBJECT
        );
    }
}
