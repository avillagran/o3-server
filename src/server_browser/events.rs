//! Per-page identity and lifecycle routing shared by inherited-pipe and desktop actors.
use super::*;
fn detached(value: &Value, session: Option<&str>) -> bool {
    (value["method"] == "Inspector.detached"
        && session.is_some_and(|session| value["sessionId"] == session))
        || (value["method"] == "Target.detachedFromTarget"
            && session.is_some_and(|session| value["params"]["sessionId"] == session))
}
pub(super) fn observe(
    value: &Value,
    session: Option<&str>,
    target: Option<&str>,
    main_frame: &mut Option<String>,
    navigation: &tokio::sync::watch::Sender<Navigation>,
) -> bool {
    if detached(value, session)
        || (value["method"] == "Target.targetDestroyed"
            && target.is_some_and(|target| value["params"]["targetId"] == target))
    {
        return true;
    }
    let scoped = session.is_some_and(|session| value["sessionId"] == session);
    let method = value["method"].as_str().unwrap_or("");
    let event = if method == "Target.targetInfoChanged" {
        target.is_some_and(|target| value["params"]["targetInfo"]["targetId"] == target)
    } else if scoped {
        match method {
            "Page.frameNavigated" => {
                let frame = &value["params"]["frame"];
                if frame.get("parentId").is_none() || frame["parentId"].as_str() == Some("") {
                    if let Some(id) = frame["id"].as_str().filter(|id| !id.is_empty()) {
                        *main_frame = Some(id.into());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            "Page.navigatedWithinDocument" | "Page.frameStartedLoading" => main_frame
                .as_ref()
                .is_some_and(|frame| value["params"]["frameId"] == *frame),
            "Page.loadEventFired" => true,
            _ => false,
        }
    } else {
        false
    };
    if event {
        navigation.send_modify(|state| {
            state.revision = state.revision.wrapping_add(1);
            if method == "Page.frameStartedLoading" {
                state.generation = state.generation.wrapping_add(1);
                state.loading = true;
            }
            if method == "Page.loadEventFired" {
                state.loading = false;
            }
        });
    }
    false
}

struct Observer {
    target: String,
    main_frame: Option<String>,
    navigation: tokio::sync::watch::Sender<Navigation>,
    done: tokio::sync::watch::Sender<bool>,
}
#[derive(Default)]
pub(super) struct Registry {
    pages: BTreeMap<String, Observer>,
    registered: bool,
}
pub(super) type Shared = std::sync::Arc<std::sync::Mutex<Registry>>;
pub(super) fn register(
    registry: &Shared,
    session: &str,
    target: &str,
) -> (
    tokio::sync::watch::Receiver<bool>,
    tokio::sync::watch::Receiver<Navigation>,
) {
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let (nav_tx, navigation) = tokio::sync::watch::channel(Navigation::default());
    let mut registry = registry.lock().unwrap();
    registry.registered = true;
    if let Some(old) = registry.pages.insert(
        session.into(),
        Observer {
            target: target.into(),
            main_frame: None,
            navigation: nav_tx,
            done: done_tx,
        },
    ) {
        old.done.send_replace(true);
    }
    (done, navigation)
}
pub(super) fn frame(registry: &Shared, session: Option<&str>, result: &Value) {
    let mut registry = registry.lock().unwrap();
    if let Some(page) = session.and_then(|session| registry.pages.get_mut(session)) {
        page.main_frame = result["frameTree"]["frame"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
    }
}
pub(super) fn remove(registry: &Shared, session: &str) {
    if let Some(page) = registry.lock().unwrap().pages.remove(session) {
        page.done.send_replace(true);
    }
}
pub(super) fn notify(registry: &Shared, value: &Value) -> bool {
    let mut registry = registry.lock().unwrap();
    let mut detached = Vec::new();
    for (session, page) in &mut registry.pages {
        if observe(
            value,
            Some(session),
            Some(&page.target),
            &mut page.main_frame,
            &page.navigation,
        ) {
            page.done.send_replace(true);
            detached.push(session.clone());
        }
    }
    for session in detached {
        registry.pages.remove(&session);
    }
    registry.registered && registry.pages.is_empty()
}
pub(super) fn stopped(registry: &Shared) {
    let mut registry = registry.lock().unwrap();
    for page in registry.pages.values() {
        page.done.send_replace(true);
    }
    registry.pages.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_detachment_preserves_sibling_identity_and_process_completion() {
        crate::test_sandbox::assert_isolated();
        let pages: Shared = Default::default();
        let (done_a, nav_a) = register(&pages, "opaque-a", "target-a");
        let (done_b, nav_b) = register(&pages, "opaque-b", "target-b");
        frame(
            &pages,
            Some("opaque-a"),
            &json!({"frameTree":{"frame":{"id":"frame-a"}}}),
        );
        assert!(!notify(
            &pages,
            &json!({"method":"Page.frameStartedLoading","sessionId":"opaque-a","params":{"frameId":"frame-a"}})
        ));
        assert!(nav_a.borrow().loading);
        assert_eq!(nav_b.borrow().revision, 0);
        assert!(!notify(
            &pages,
            &json!({"method":"Target.detachedFromTarget","params":{"sessionId":"opaque-a"}})
        ));
        assert!(*done_a.borrow());
        assert!(!*done_b.borrow());
        assert!(notify(
            &pages,
            &json!({"method":"Target.targetDestroyed","params":{"targetId":"target-b"}})
        ));
        assert!(*done_b.borrow());
    }
}
