//! Focused source-only entry regression cases. Run only in the private test container.
use super::*;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::oneshot;

// These cases isolate exact deadline/provenance behavior. The production
// source-lifetime gate and ledger accounting have separate service tests.
fn open_entry() -> SourceEntryGate {
    SourceEntryGate::new(Arc::new(|| Ok(())))
}

#[tokio::test]
async fn source_expired_queue_never_mints_a_callback_after_its_predecessor() {
    crate::test_sandbox::assert_isolated();
    let control = Control::new(Some("agent".into()));
    let (entered, started) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let first = control.agent_admitted("agent", None, move |_| async move {
        entered.send(()).unwrap();
        released.await.unwrap();
        Ok(Value::Null)
    });
    started.await.unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let marker = called.clone();
    let queued = control.source_agent_admitted(
        "agent",
        tokio::time::Instant::now() - Duration::from_nanos(1),
        open_entry(),
        move |_| async move {
            marker.store(true, Ordering::Release);
            Ok(Value::Null)
        },
    );
    release.send(()).unwrap();
    first.wait().await.unwrap();
    assert_eq!(
        queued.wait().await,
        Err(json!({
            "name":"TimeoutError",
            "message":"This browser request expired before it could start."
        }))
    );
    assert!(!called.load(Ordering::Acquire));
    control.close().wait().await.unwrap();
}

#[tokio::test]
async fn source_ownership_and_generation_guards_precede_its_exact_deadline() {
    crate::test_sandbox::assert_isolated();
    let control = Control::new(Some("agent".into()));
    let called = Arc::new(AtomicBool::new(false));
    let marker = called.clone();
    let mismatch = control.source_agent_admitted(
        "other",
        tokio::time::Instant::now() - Duration::from_nanos(1),
        open_entry(),
        move |_| async move {
            marker.store(true, Ordering::Release);
            Ok(Value::Null)
        },
    );
    let refused = mismatch.wait().await.unwrap_err();
    assert_eq!(refused["_tag"], "BrowserControlInterrupted");
    assert_ne!(refused["name"], "TimeoutError");
    assert!(!called.load(Ordering::Acquire));

    let (entered, started) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let first = control.agent_admitted("agent", None, move |_| async move {
        entered.send(()).unwrap();
        released.await.unwrap();
        Ok(Value::Null)
    });
    started.await.unwrap();
    let marker = called.clone();
    let queued = control.source_agent_admitted(
        "agent",
        tokio::time::Instant::now() - Duration::from_nanos(1),
        open_entry(),
        move |_| async move {
            marker.store(true, Ordering::Release);
            Ok(Value::Null)
        },
    );
    let handoff = control.take("human");
    release.send(()).unwrap();
    first.wait().await.unwrap();
    let refused = queued.wait().await.unwrap_err();
    assert_eq!(refused["_tag"], "BrowserControlInterrupted");
    assert_eq!(refused["reason"], "interrupted");
    assert!(!called.load(Ordering::Acquire));
    handoff.wait().await.unwrap();
    control.close().wait().await.unwrap();
}

#[tokio::test]
async fn source_entered_tail_survives_observer_drop_handoff_and_its_admission_deadline() {
    crate::test_sandbox::assert_isolated();
    let control = Control::new(Some("agent".into()));
    let (entered, started) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let marker = finished.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let action =
        control.source_agent_admitted("agent", deadline, open_entry(), move |context| async move {
            entered.send(context.clone()).unwrap();
            released.await.unwrap();
            assert!(context.is_active());
            marker.store(true, Ordering::Release);
            Ok(Value::Null)
        });
    let observer = tokio::spawn(action.wait());
    let context = started.await.unwrap();
    assert_eq!(context.source_agent(), Some("agent"));
    observer.abort();
    assert!(observer.await.unwrap_err().is_cancelled());
    let handoff = control.take("human");
    tokio::time::sleep_until(deadline + Duration::from_nanos(1)).await;
    assert!(control.owns_context(&context));
    assert!(!finished.load(Ordering::Acquire));
    release.send(()).unwrap();
    handoff.wait().await.unwrap();
    control.close().wait().await.unwrap();
    assert!(finished.load(Ordering::Acquire));
    assert!(!context.is_active());
}

#[tokio::test]
async fn generic_agent_and_observer_contexts_cannot_supply_a_source_agent_seal() {
    crate::test_sandbox::assert_isolated();
    let control = Control::new(Some("agent".into()));
    let agent = control.agent_admitted("agent", None, move |context| async move {
        assert!(context.source_agent().is_none());
        Ok(Value::Null)
    });
    agent.wait().await.unwrap();
    let observer = control.observe_admitted(move |context| async move {
        assert!(context.source_agent().is_none());
        Ok(Value::Null)
    });
    observer.wait().await.unwrap();
    control.close().wait().await.unwrap();
}
