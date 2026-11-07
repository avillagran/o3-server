//! Real Tokio ownership/drain tests with inert callbacks and private test admission.
use o3_server::preview_control::runtime;
#[path = "support/sandbox.rs"]
mod sandbox;
use runtime::Control;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;
async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("owned control fixture deadline")
}
#[tokio::test]
async fn reservation_invalidates_waiting_work_and_canceled_receivers_cannot_drop_running_actions() {
    sandbox::assert_isolated();
    let control = Control::new(Some("A".into()));
    let mut changed = control.subscribe();
    assert_eq!(control.snapshot().agent_id.as_deref(), Some("A"));
    let (started, start) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let running = control.agent("A", None, move || async move {
        started.send(()).unwrap();
        gate.await.unwrap();
        Ok(json!("first"))
    });
    bounded(start).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let waiting = control.agent("A", None, move || async move {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Null)
    });
    let waiting_observer = waiting.clone();
    drop(waiting);
    let taking = control.take("human");
    assert_eq!(control.snapshot().controller.as_deref(), Some("human"));
    assert_eq!(control.snapshot().generation, 1);
    bounded(changed.changed()).await.unwrap();
    assert_eq!(changed.borrow().controller.as_deref(), Some("human"));
    release.send(()).unwrap();
    assert_eq!(bounded(running.wait()).await.unwrap(), json!("first"));
    assert_eq!(
        bounded(waiting_observer.wait()).await.unwrap_err()["reason"],
        "interrupted"
    );
    bounded(taking.wait()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        bounded(
            control
                .agent("A", None, || async { Ok(Value::Null) })
                .wait()
        )
        .await
        .unwrap_err()["reason"],
        "humanControl"
    );
    bounded(
        control
            .disconnect("human", || async { Ok(Value::Null) })
            .wait(),
    )
    .await
    .unwrap();
    assert_eq!(control.snapshot().generation, 2);
    assert!(control.snapshot().controller.is_none());
    bounded(control.close().wait()).await.unwrap();
    bounded(control.shutdown()).await.unwrap();
}
#[tokio::test]
async fn dropped_call_still_runs_and_independent_observation_does_not_wait_for_the_queue() {
    sandbox::assert_isolated();
    let control = Control::new(None);
    let (started, start) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let completed = Arc::new(AtomicUsize::new(0));
    let done = completed.clone();
    let action = control.system(move || async move {
        started.send(()).unwrap();
        gate.await.unwrap();
        done.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Null)
    });
    bounded(start).await.unwrap();
    drop(action);
    let read = tokio::spawn(control.observe(|| async { Ok(json!("read")) }).wait());
    assert_eq!(bounded(read).await.unwrap().unwrap(), json!("read"));
    release.send(()).unwrap();
    bounded(control.close().wait()).await.unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    bounded(control.shutdown()).await.unwrap();
}
#[tokio::test]
async fn pending_navigation_blocks_later_work_but_handoff_tracking_only_blocks_release() {
    sandbox::assert_isolated();
    let control = Control::new(None);
    let (started, start) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let first = control.system(move || async move {
        started.send(()).unwrap();
        gate.await.unwrap();
        Ok(Value::Null)
    });
    bounded(start).await.unwrap();
    let (navigation, nav) = oneshot::channel();
    control.track(async move {
        let _ = nav.await;
    });
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let second = control.system(move || async move {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Null)
    });
    release.send(()).unwrap();
    bounded(first.wait()).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), second.clone().wait())
            .await
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    navigation.send(()).unwrap();
    bounded(second.wait()).await.unwrap();
    bounded(control.take("human").wait()).await.unwrap();
    let (handoff, nav) = oneshot::channel();
    control.track_until_handoff(async move {
        let _ = nav.await;
    });
    bounded(control.human("human", || async { Ok(Value::Null) }).wait())
        .await
        .unwrap();
    let release = control.release("human", || async { Ok(Value::Null) });
    assert!(control.snapshot().controller.is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), release.clone().wait())
            .await
            .is_err()
    );
    handoff.send(()).unwrap();
    bounded(release.wait()).await.unwrap();
    bounded(control.shutdown()).await.unwrap();
}
#[tokio::test]
async fn adjacent_latest_inputs_share_owned_work_without_replacing_running_or_intervening_actions()
{
    sandbox::assert_isolated();
    let control = Control::new(None);
    bounded(control.take("human").wait()).await.unwrap();
    let (started, start) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let blocker = control.human("human", move || async move {
        started.send(()).unwrap();
        gate.await.unwrap();
        Ok(Value::Null)
    });
    bounded(start).await.unwrap();
    let log = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = log.clone();
    let first = control.human_latest(
        "human",
        "hover",
        json!(1),
        move |input| async move {
            observed.lock().unwrap().push(json!(["first", input]));
            Ok(Value::Null)
        },
        |_, next| next,
    );
    let observed = log.clone();
    let second = control.human_latest(
        "human",
        "hover",
        json!(2),
        move |input| async move {
            observed.lock().unwrap().push(json!(["second", input]));
            Ok(Value::Null)
        },
        |old, next| json!(old.as_i64().unwrap() + next.as_i64().unwrap()),
    );
    assert_eq!(first.action_id(), second.action_id());
    drop(first);
    let observed = log.clone();
    let middle = control.system(move || async move {
        observed.lock().unwrap().push(json!("middle"));
        Ok(Value::Null)
    });
    let observed = log.clone();
    let third = control.human_latest(
        "human",
        "hover",
        json!(4),
        move |input| async move {
            observed.lock().unwrap().push(json!(["third", input]));
            Ok(Value::Null)
        },
        |_, next| next,
    );
    assert_ne!(second.action_id(), third.action_id());
    release.send(()).unwrap();
    bounded(blocker.wait()).await.unwrap();
    bounded(second.wait()).await.unwrap();
    bounded(middle.wait()).await.unwrap();
    bounded(third.wait()).await.unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        vec![json!(["second", 3]), json!("middle"), json!(["third", 4])]
    );
    bounded(control.shutdown()).await.unwrap();
}
#[tokio::test]
async fn close_and_shutdown_reserve_immediately_but_drain_owned_actions_before_completion() {
    sandbox::assert_isolated();
    let control = Control::new(Some("A".into()));
    let (started, start) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let running = control.agent("A", None, move || async move {
        started.send(()).unwrap();
        gate.await.unwrap();
        Ok(Value::Null)
    });
    bounded(start).await.unwrap();
    let closing = control.close();
    assert!(control.snapshot().closed);
    assert_eq!(control.snapshot().generation, 1);
    drop(closing);
    drop(running);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), control.shutdown())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    let owner = control.clone();
    bounded(tokio::spawn(async move { owner.shutdown().await }))
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn expired_queued_action_never_starts_after_its_predecessor_drains() {
    sandbox::assert_isolated();
    let control = Control::new(Some("A".into()));
    let (started, start) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let blocker = control.system(move || async move {
        started.send(()).unwrap();
        gate.await.unwrap();
        Ok(Value::Null)
    });
    bounded(start).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let invoked = calls.clone();
    let action = control.agent("A", Some(tokio::time::Instant::now()), move || async move {
        invoked.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Null)
    });
    release.send(()).unwrap();
    bounded(blocker.wait()).await.unwrap();
    assert_eq!(
        bounded(action.wait()).await.unwrap_err()["name"],
        "TimeoutError"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    bounded(control.shutdown()).await.unwrap();
}

#[tokio::test]
async fn final_owner_drop_closes_waiting_admission_and_keeps_the_running_callback_owned() {
    sandbox::assert_isolated();
    let control = Control::new(Some("A".into()));
    let (started, start) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let finished = Arc::new(AtomicUsize::new(0));
    let completed = finished.clone();
    let running = control.agent("A", None, move || async move {
        started.send(()).unwrap();
        gate.await.unwrap();
        completed.fetch_add(1, Ordering::SeqCst);
        Ok(json!("retained"))
    });
    bounded(start).await.unwrap();
    let invoked = Arc::new(AtomicUsize::new(0));
    let calls = invoked.clone();
    let waiting = control.agent("A", None, move || async move {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Null)
    });
    drop(control);
    release.send(()).unwrap();
    assert_eq!(bounded(running.wait()).await.unwrap(), json!("retained"));
    assert_eq!(
        bounded(waiting.wait()).await.unwrap_err()["reason"],
        "closed"
    );
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert_eq!(invoked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejected_self_capturing_callback_disposes_its_final_owner_outside_the_state_lock() {
    sandbox::assert_isolated();
    let control = Control::new(Some("A".into()));
    let captured = control.clone();
    let rejected = control.agent("B", None, move || async move {
        drop(captured);
        Ok(Value::Null)
    });
    drop(control);
    assert_eq!(
        bounded(rejected.wait()).await.unwrap_err()["reason"],
        "agentMismatch"
    );
}

#[tokio::test]
async fn tracking_after_shutdown_disposes_reentrant_future_outside_the_state_lock() {
    sandbox::assert_isolated();
    struct Probe(Control, Arc<AtomicUsize>);
    impl Drop for Probe {
        fn drop(&mut self) {
            assert!(self.0.snapshot().closed);
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }
    let control = Control::new(None);
    bounded(control.shutdown()).await.unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    let probe = Probe(control.clone(), dropped.clone());
    control.track(async move { drop(probe) });
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn admitted_context_is_owner_bound_and_drains_after_take_and_close() {
    sandbox::assert_isolated();
    let control = Control::new(Some("A".into()));
    let foreign = Control::new(Some("A".into()));
    let (entered, context) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let running = control.agent_admitted("A", None, move |context| async move {
        entered.send(context).unwrap();
        gate.await.unwrap();
        Ok(json!("written"))
    });
    let context = bounded(context).await.unwrap();
    assert_eq!(context.action_id(), running.action_id());
    assert!(control.owns_context(&context));
    assert!(!foreign.owns_context(&context));
    let generation = context.generation();
    let invoked = Arc::new(AtomicUsize::new(0));
    let count = invoked.clone();
    let queued = control.agent_admitted("A", None, move |_| async move {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Value::Null)
    });
    let take = control.take("human");
    assert_ne!(generation, control.snapshot().generation);
    assert!(control.owns_context(&context));
    let close = control.close();
    assert!(control.snapshot().closed);
    assert!(control.owns_context(&context));
    release.send(()).unwrap();
    assert_eq!(bounded(running.wait()).await.unwrap(), json!("written"));
    assert!(!context.is_active());
    assert!(!control.owns_context(&context));
    assert!(bounded(queued.wait()).await.is_err());
    assert_eq!(invoked.load(Ordering::SeqCst), 0);
    let _ = bounded(take.wait()).await;
    bounded(close.wait()).await.unwrap();
    bounded(control.shutdown()).await.unwrap();
    bounded(foreign.shutdown()).await.unwrap();
}

#[tokio::test]
async fn observer_admission_captures_submission_generation_before_first_poll() {
    sandbox::assert_isolated();
    let control = Control::new(None);
    let generation = control.snapshot().generation;
    let (entered, context) = oneshot::channel();
    let observe = control.observe_admitted(move |context| async move {
        entered.send(context).unwrap();
        Ok(Value::Null)
    });
    let take = control.take("human");
    assert_ne!(generation, control.snapshot().generation);
    let context = bounded(context).await.unwrap();
    assert_eq!(context.generation(), generation);
    bounded(observe.wait()).await.unwrap();
    bounded(take.wait()).await.unwrap();
    assert!(!context.is_active());
    bounded(control.shutdown()).await.unwrap();
}
