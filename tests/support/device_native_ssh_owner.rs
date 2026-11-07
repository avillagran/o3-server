use super::ssh_fixture::Fixture;
use o3_server::{device_native_ssh_owner::SshOwner, device_native_ssh_transport::ForwardOptions};
use std::{sync::atomic::Ordering, time::Duration};
impl Fixture {
    fn owner(&self) -> SshOwner {
        SshOwner::new(self.cli.clone()).with_forward_options(ForwardOptions {
            ready_timeout: Duration::from_millis(400),
            probe_interval: Duration::from_millis(30),
            probe_timeout: Duration::from_millis(50),
        })
    }
}
#[tokio::test]
async fn actual_private_cli_forward_retry_hub_and_receipt_stop() {
    let fixture = Fixture::make(2).await;
    let owner = fixture.owner();
    let (first, second) = tokio::join!(owner.ensure_ready(), owner.ensure_ready());
    let first = first.unwrap();
    assert_eq!(first.generation, second.unwrap().generation);
    assert!(first.hub.client.list().await.unwrap().devices.is_empty());
    let trace = fixture.trace();
    assert_eq!(
        trace
            .iter()
            .filter(|entry| entry["mode"] == "forward")
            .count(),
        3
    );
    assert_eq!(
        trace
            .iter()
            .filter(|entry| entry["mode"] == "start")
            .count(),
        3
    );
    owner.stop().await.unwrap();
    assert!(owner.current().is_none());
    assert!(!first.hub.process.alive());
    let trace = fixture.trace();
    let stopped = trace.iter().find(|entry| entry["mode"] == "stop").unwrap();
    assert_eq!(stopped["receipt"], "c".repeat(48));
    assert_eq!(stopped["owner"], fixture.cli.owner);
}
#[tokio::test]
async fn three_unhealthy_probes_reconnect_and_drop_closes_forward_with_live_ready_handle() {
    let fixture = Fixture::make(0).await;
    let owner = fixture.owner();
    let first = owner.ensure_ready().await.unwrap();
    fixture.healthy.store(false, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(3), first.hub.process.wait())
        .await
        .unwrap();
    fixture.healthy.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if owner
                .current()
                .is_some_and(|ready| ready.generation > first.generation)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let current = owner.current().unwrap();
    drop(owner);
    tokio::time::timeout(Duration::from_secs(3), current.hub.process.wait())
        .await
        .unwrap();
    assert!(!current.hub.process.alive());
    // Explicit teardown remains available with the retained receipt after local owner release.
    fixture.cli.stop(&"c".repeat(48)).await.unwrap();
}
