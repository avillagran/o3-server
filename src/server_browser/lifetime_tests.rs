//! Actual private pipe processes; removal requests are recorded, never executed.
use super::*;
use crate::store::State;
use std::sync::{atomic::Ordering, Arc};

#[derive(Default)]
struct Recording(std::sync::Mutex<Vec<PathBuf>>);
impl crate::cleanup::Removal for Recording {
    fn execute(&self, plan: &crate::cleanup::RemovalPlan) -> std::io::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(plan.operations().last().unwrap().path.clone());
        Ok(())
    }
}
fn state() -> (Arc<State>, Arc<Recording>) {
    crate::test_sandbox::assert_isolated();
    let root = std::env::temp_dir().join(format!(
        "browser-lifetime-{}",
        crate::store::random_id().unwrap()
    ));
    let state = Arc::new(State::open(&root).unwrap());
    let browser = &state.streams.p2.preview.browser;
    *browser.fixture_binary.lock().unwrap() =
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/browser-cdp.py"));
    let recording = Arc::new(Recording::default());
    browser.set_profile_removal(recording.clone());
    (state, recording)
}
async fn open(state: &State, thread: &str, owner: Option<&str>, profile: &str) -> Page {
    let input = json!({"threadId":thread,"runtime":"server","profileId":profile,"url":"https://example.test/"});
    let snapshot = if let Some(owner) = owner {
        crate::preview::open_agent(state, &input, owner)
            .await
            .unwrap()
    } else {
        crate::preview::call(state, "preview.open", &input)
            .await
            .unwrap()
    };
    state
        .streams
        .p2
        .preview
        .browser
        .page(thread, snapshot["tabId"].as_str().unwrap())
        .unwrap()
}
async fn wait(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn rejected_agent_open_does_not_sweep_an_idle_tab() {
    let (state, _) = state();
    let page = open(&state, "thread", Some("agent"), "agent:invalid-url").await;
    page.tab.activity.used.store(
        crate::store::now_ms() - crate::browser_lifetime::IDLE_MS - 1,
        Ordering::Relaxed,
    );
    let error = crate::preview::open_agent(
        &state,
        &json!({"threadId":"thread","runtime":"server","url":"javascript:alert(1)"}),
        "agent",
    )
    .await
    .unwrap_err();
    assert_eq!(error["_tag"], "PreviewInvalidUrlError");
    assert!(state.streams.p2.preview.browser.current(&page));
    assert!(state.streams.p2.preview.pending_lifetime().is_empty());
    crate::preview::shutdown(&state).await;
}
#[tokio::test]
async fn strict_idle_boundary_reaps_only_agent_tabs_and_retains_recorded_profiles() {
    let (state, recording) = state();
    let agent = open(&state, "thread", Some("agent"), "agent:idle-fixture").await;
    let human = open(&state, "thread", None, "default").await;
    let used = agent.tab.activity.used.load(Ordering::Relaxed);
    state
        .streams
        .p2
        .preview
        .sweep_idle_at(&state, used + crate::browser_lifetime::IDLE_MS)
        .await;
    assert!(state.streams.p2.preview.browser.current(&agent));
    state
        .streams
        .p2
        .preview
        .sweep_idle_at(&state, used + crate::browser_lifetime::IDLE_MS + 1)
        .await;
    assert!(!state.streams.p2.preview.browser.current(&agent));
    assert!(
        *agent.tab.process_done.borrow(),
        "close waits for its owned process"
    );
    assert!(
        state.streams.p2.preview.browser.current(&human),
        "unclaimed tabs have no agent expiry"
    );
    let plans = recording.0.lock().unwrap().clone();
    assert_eq!(plans.len(), 1);
    assert!(
        plans[0].join("fixture.pid").is_file(),
        "recording preserves the original profile tree"
    );
    crate::preview::shutdown(&state).await;
}
#[tokio::test]
async fn a_read_only_viewer_prevents_expiry_until_its_real_finalizer_releases_visibility() {
    let (state, _) = state();
    let page = open(&state, "thread", Some("agent"), "agent:viewer-fixture").await;
    let now = page.tab.activity.used.load(Ordering::Relaxed) + crate::browser_lifetime::IDLE_MS + 1;
    let viewer = state
        .streams
        .p2
        .preview
        .browser
        .claim_viewer(page.clone(), "read-only-viewer".into())
        .unwrap();
    assert!(
        page.control().snapshot().controller.is_none(),
        "visibility does not grant control"
    );
    state.streams.p2.preview.sweep_idle_at(&state, now).await;
    assert!(state.streams.p2.preview.browser.current(&page));
    drop(viewer);
    wait(|| {
        !state
            .streams
            .p2
            .preview
            .browser
            .visible(page.key().0, page.key().1)
    })
    .await;
    state.streams.p2.preview.sweep_idle_at(&state, now).await;
    assert!(!state.streams.p2.preview.browser.current(&page));
    crate::preview::shutdown(&state).await;
}
#[tokio::test]
async fn stale_idle_plan_rechecks_real_activity_and_viewers_before_reserving_close() {
    let (state, _) = state();
    let page = open(&state, "thread", Some("agent"), "agent:stale-fixture").await;
    let now = crate::store::now_ms();
    page.tab.activity.used.store(
        now - crate::browser_lifetime::IDLE_MS - 1,
        Ordering::Relaxed,
    );
    let planned = state
        .streams
        .p2
        .preview
        .browser
        .idle_candidates(now, true)
        .pop()
        .unwrap();
    state
        .streams
        .p2
        .preview
        .browser
        .mark_used("thread", "agent", None);
    assert!(
        !state
            .streams
            .p2
            .preview
            .browser
            .reserve_idle(&planned, now, true),
        "fresh completion invalidates the older plan"
    );
    page.tab.activity.used.store(
        now - crate::browser_lifetime::IDLE_MS - 1,
        Ordering::Relaxed,
    );
    let viewer = state
        .streams
        .p2
        .preview
        .browser
        .claim_viewer(page.clone(), "late-viewer".into())
        .unwrap();
    assert!(
        !state
            .streams
            .p2
            .preview
            .browser
            .reserve_idle(&planned, now, true),
        "fresh visibility invalidates the older plan"
    );
    drop(viewer);
    wait(|| {
        !state
            .streams
            .p2
            .preview
            .browser
            .visible(page.key().0, page.key().1)
    })
    .await;
    assert!(state
        .streams
        .p2
        .preview
        .browser
        .reserve_idle(&planned, now, true));
    assert!(
        state
            .streams
            .p2
            .preview
            .browser
            .claim_viewer(page.clone(), "too-late".into())
            .is_err(),
        "a reserved close cannot admit a late viewer"
    );
    crate::preview::call(
        &state,
        "preview.close",
        &json!({"threadId":page.key().0,"tabId":page.key().1}),
    )
    .await
    .unwrap();
    crate::preview::shutdown(&state).await;
}
#[tokio::test]
async fn activity_marks_owned_tabs_and_explicit_reads_only_in_the_requested_thread() {
    let (state, _) = state();
    let first = open(&state, "thread", Some("agent"), "default").await;
    let second = open(&state, "thread", Some("agent"), "default").await;
    let foreign = open(&state, "thread", Some("other"), "default").await;
    let other_thread = open(&state, "other-thread", Some("agent"), "default").await;
    for page in [&first, &second, &foreign, &other_thread] {
        page.tab.activity.used.store(1, Ordering::Relaxed);
    }
    state
        .streams
        .p2
        .preview
        .browser
        .mark_used("thread", "agent", Some(foreign.key().1));
    for page in [&first, &second, &foreign] {
        assert!(page.tab.activity.used.load(Ordering::Relaxed) > 1);
    }
    assert_eq!(other_thread.tab.activity.used.load(Ordering::Relaxed), 1);
    state.streams.p2.preview.start_idle_worker(&state);
    tokio::time::timeout(Duration::from_secs(2), crate::preview::shutdown(&state))
        .await
        .unwrap();
    assert!(*first.tab.process_done.borrow());
}
#[tokio::test]
async fn native_agent_budget_is_eight_and_server_budget_is_thirty_two() {
    let (state, _) = state();
    let mut opened = Vec::new();
    for index in 0..32 {
        opened.push(
            open(
                &state,
                "thread",
                Some(&format!("agent-{}", index / 8)),
                "default",
            )
            .await,
        );
    }
    let failed = crate::preview::open_agent(
        &state,
        &json!({"threadId":"thread","runtime":"server","profileId":"default"}),
        "fresh",
    )
    .await
    .unwrap_err();
    assert_eq!(failed["reason"], "tabLimit");
    assert_eq!(
        state.streams.p2.preview.browser.thread_tabs("thread").len(),
        32
    );
    let removed = &opened[0];
    crate::preview::call(
        &state,
        "preview.close",
        &json!({"threadId":"thread","tabId":removed.key().1}),
    )
    .await
    .unwrap();
    let own_full = crate::preview::open_agent(
        &state,
        &json!({"threadId":"thread","runtime":"server","profileId":"default"}),
        "agent-1",
    )
    .await
    .unwrap_err();
    assert_eq!(
        own_full["reason"], "tabLimit",
        "free server room cannot bypass the independent agent budget"
    );
    let fresh = open(&state, "thread", Some("fresh"), "default").await;
    assert!(state.streams.p2.preview.browser.current(&fresh));
    crate::preview::shutdown(&state).await;
}
#[tokio::test]
async fn cancelled_launch_keeps_its_capacity_until_the_owned_setup_and_cleanup_settle() {
    use std::os::unix::fs::PermissionsExt;
    let (state, _) = state();
    let fixture = state.dir.join("slow-browser.py");
    let source = include_str!("../../tests/fixtures/browser-cdp.py")
        .replace("import json,os,sys", "import json,os,sys,time")
        .replace(
            "        os.write(4,",
            "        if v['method']=='Target.createTarget':time.sleep(0.3)\n        os.write(4,",
        );
    std::fs::write(&fixture, source).unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o700)).unwrap();
    *state
        .streams
        .p2
        .preview
        .browser
        .fixture_binary
        .lock()
        .unwrap() = Some(fixture);
    let mut tasks = Vec::new();
    for index in 0..8 {
        let state = state.clone();
        tasks.push(tokio::spawn(async move {crate::preview::open_agent(&state,&json!({"threadId":"thread","runtime":"server","profileId":format!("agent:pending-{index}")}),"agent").await}));
    }
    wait(|| state.streams.p2.preview.pending_lifetime().len() == 8).await;
    let victim = tasks.remove(0);
    victim.abort();
    let _ = victim.await;
    let failed = crate::preview::open_agent(
        &state,
        &json!({"threadId":"thread","runtime":"server","profileId":"agent:ninth"}),
        "agent",
    )
    .await
    .unwrap_err();
    assert_eq!(
        failed["reason"], "tabLimit",
        "logical cancellation cannot release an unsettled physical reservation"
    );
    for task in tasks {
        let _ = task.await.unwrap();
    }
    wait(|| state.streams.p2.preview.pending_lifetime().is_empty()).await;
    let fresh = open(&state, "thread", Some("agent"), "agent:ninth").await;
    assert!(state.streams.p2.preview.browser.current(&fresh));
    crate::preview::shutdown(&state).await;
}
