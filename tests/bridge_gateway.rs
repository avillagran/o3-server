#![cfg(feature = "bridge")]
use omabridge::{
    gateway,
    manage::{EmbeddedBackend, ServeBackend},
    Client, NetOptions, Node, NodeConfig, Store,
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[test]
fn rust_client_over_test_ca_gateway_local_and_quic() {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap().block_on(async {
        tokio::time::timeout(Duration::from_secs(60), async {
            let root = std::env::temp_dir().join(format!("gw-{}",o3_server::store::random_id().unwrap()));
            std::fs::create_dir_all(&root).unwrap();
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            let pem = root.join("cert.pem"); let key = root.join("key.pem");
            std::fs::write(&pem, cert.cert.pem()).unwrap();
            std::fs::write(&key, cert.signing_key.serialize_pem()).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
            let mut roots = rustls::RootCertStore::empty(); roots.add(cert.cert.der().clone()).unwrap();
            let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth();
            let connector = tokio_rustls::TlsConnector::from(Arc::new(tls));
            let state = Arc::new(o3_server::store::State::open(&root.join("profile")).unwrap());
            let host = Store::open(&root.join("h"), "host").unwrap();
            let backend = EmbeddedBackend::new(host.clone(), vec!["t3".into()]).unwrap();
            backend.unpublish().unwrap();
            drop(backend);
            assert!(!EmbeddedBackend::new(host.clone(), vec!["t3".into()]).unwrap().allows("t3"));
            let backend = EmbeddedBackend::new(host.clone(), vec!["t3".into()]).unwrap();
            backend.publish(&json!({"t3":true})).unwrap();
            let port = || { let socket=std::net::TcpListener::bind("127.0.0.1:0").unwrap(); socket.local_addr().unwrap().port() };
            let local_port=port();
            let config_path=root.join("gateway.json");
            std::fs::write(&config_path, json!({"bind":"127.0.0.1","certificates":{"localhost":[pem,key]},"gateways":[{"host":"localhost","port":local_port,"peer":host.id().unwrap(),"service":"t3"}]}).to_string()).unwrap();
            let node=o3_server::bridge::start_hosted(host.root(),state.clone(),NetOptions::loopback(),Some(&config_path),false).await.unwrap();
            let remote=Store::open(&root.join("r"),"gateway").unwrap();
            let client=Client::new(remote.clone(),NetOptions::loopback()).await.unwrap();
            let now=omabridge::unix_time().unwrap();
            let invite=omabridge::Invitation{version:1,address:node.addr(),token:host.create_invite(&["t3".into()],Duration::from_secs(60),now).unwrap(),name:"host".into(),expires:now+60,manage:false};
            client.pair(&invite,"gateway",now).await.unwrap(); host.approve(&remote.id().unwrap(),now).unwrap(); client.pair(&invite,"gateway",now).await.unwrap();
            host.set_services(&remote.id().unwrap(), &["t3".into()], &["t3".into()]).unwrap();
            client.close().await;
            let remote_port=port();
            let entry=gateway::Gateway{host:"localhost".into(),port:remote_port,peer:host.id().unwrap(),service:"t3".into()};
            gateway::save(remote.root(), &[entry]).unwrap();
            let mut config=NodeConfig::embedded(); config.gateways=true;
            config.gateway_runtime=Some(gateway::GatewayRuntime{bind:"127.0.0.1".parse().unwrap(),certificates:BTreeMap::from([("localhost".into(),(pem,key))]),local:BTreeMap::new(),backend:Arc::new(EmbeddedBackend::new(remote.clone(),vec![]).unwrap())});
            let gateway_node=Node::start_configured(remote.clone(),BTreeMap::new(),NetOptions::loopback(),remote.reserve_runtime().unwrap(),None,None,None,config).await.unwrap();
            for (store,port) in [(&host,local_port),(&remote,remote_port)] {
                while !gateway::status(store.root()).values().any(|s| s.running) { tokio::time::sleep(Duration::from_millis(50)).await; }
                // Rust protocol client through a CA-validating TLS transport adapter.
                let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let base=format!("http://{}/", listener.local_addr().unwrap());
                let connector=connector.clone();
                let adapter=tokio::spawn(async move {
                    let mut workers=tokio::task::JoinSet::new();
                    loop { tokio::select! {
                        accepted=listener.accept()=> { let (mut socket,_)=accepted.unwrap(); let connector=connector.clone(); workers.spawn(async move {
                            let upstream=tokio::net::TcpStream::connect(("127.0.0.1",port)).await.unwrap();
                            let mut tls=connector.connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(),upstream).await.unwrap();
                            let _=tokio::io::copy_bidirectional(&mut socket,&mut tls).await;
                        }); },
                        _=workers.join_next(), if !workers.is_empty()=>{}
                    } }
                });
                let environment=state.environment_id.clone();
                let pairing=state.issue_pairing(Some("gateway-test"),120000).unwrap()["credential"].as_str().unwrap().to_owned();
                tokio::task::spawn_blocking(move || {
                    let options=t3_protocol::ConnectionOptions::default();
                    t3_protocol::verify_environment(&base,&environment,&options).unwrap();
                    let paired=t3_protocol::pair_bearer(&base,&pairing,None,&Default::default(),&options).unwrap();
                    assert!(t3_protocol::pair_bearer(&base,&pairing,None,&Default::default(),&options).is_err());
                    let connection=paired.connection(&options).unwrap();
                    assert_eq!(connection.rpc("server.getConfig",json!({})).unwrap()["environment"]["environmentId"],environment);
                }).await.unwrap();
                adapter.abort(); let _=adapter.await;
            }
            gateway_node.shutdown().await; node.shutdown().await;
            // A restart retains both publication and identity, with no units generated.
            assert!(backend.allows("t3"));
            let restarted=o3_server::bridge::start(host.root(),state,NetOptions::loopback()).await.unwrap();
            assert_eq!(host.id().unwrap(), restarted.addr().id.to_string());
            restarted.shutdown().await;
            // Fixture trees are retained for private container teardown.
        }).await.unwrap();
    });
}
