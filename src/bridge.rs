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
    let store = Store::open(path, "o3-server")?;
    let reservation = store.reserve_runtime()?;
    let backend = Arc::new(
        omabridge::manage::EmbeddedBackend::new(store.clone(), vec!["t3".into()])
            .map_err(anyhow::Error::msg)?,
    );
    let mut config = NodeConfig::embedded();
    config.backend = Some(backend);
    let mut node = Node::start_configured(
        store.clone(),
        BTreeMap::from([("t3".into(), Target::Local(Arc::new(Service(state.clone()))))]),
        options,
        reservation,
        None,
        None,
        Some(Arc::new(Issuer(state.clone()))),
        config,
    )
    .await?;
    store.publish_endpoint(&node.addr(), &["t3".into()])?;
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
