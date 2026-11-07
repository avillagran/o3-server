//! Per-page identity and lifecycle routing shared by inherited-pipe and desktop actors.
use super::*;
use crate::preview::network;
#[derive(Clone, Copy)]
enum PageEvent {
    Start,
    Load,
    Changed,
}
fn classify(
    value: &Value,
    session: Option<&str>,
    target: Option<&str>,
    main_frame: &mut Option<String>,
) -> Result<Option<PageEvent>, ()> {
    if (value["method"] == "Inspector.detached"
        && session.is_some_and(|session| value["sessionId"] == session))
        || (value["method"] == "Target.detachedFromTarget"
            && session.is_some_and(|session| value["params"]["sessionId"] == session))
        || (value["method"] == "Target.targetDestroyed"
            && target.is_some_and(|target| value["params"]["targetId"] == target))
    {
        return Err(());
    }
    let scoped = session.is_some_and(|session| value["sessionId"] == session);
    let method = value["method"].as_str().unwrap_or("");
    Ok(if method == "Target.targetInfoChanged" {
        target
            .filter(|target| value["params"]["targetInfo"]["targetId"] == *target)
            .map(|_| PageEvent::Changed)
    } else if scoped {
        match method {
            "Page.frameNavigated" => {
                let frame = &value["params"]["frame"];
                if frame.get("parentId").is_none() || frame["parentId"].as_str() == Some("") {
                    frame["id"].as_str().filter(|id| !id.is_empty()).map(|id| {
                        *main_frame = Some(id.into());
                        PageEvent::Changed
                    })
                } else {
                    None
                }
            }
            "Page.navigatedWithinDocument" => main_frame
                .as_ref()
                .filter(|frame| value["params"]["frameId"] == frame.as_str())
                .map(|_| PageEvent::Changed),
            "Page.frameStartedLoading" => main_frame
                .as_ref()
                .filter(|frame| value["params"]["frameId"] == frame.as_str())
                .map(|_| PageEvent::Start),
            "Page.loadEventFired" => Some(PageEvent::Load),
            _ => None,
        }
    } else {
        None
    })
}
#[cfg(test)]
pub(super) fn observe(
    value: &Value,
    session: Option<&str>,
    target: Option<&str>,
    main_frame: &mut Option<String>,
    navigation: &tokio::sync::watch::Sender<Navigation>,
) -> bool {
    match classify(value, session, target, main_frame) {
        Err(()) => true,
        Ok(None) => false,
        Ok(Some(event)) => {
            navigation.send_modify(|state| {
                state.revision = state.revision.wrapping_add(1);
                match event {
                    PageEvent::Start => {
                        state.generation += 1;
                        state.loading = true;
                    }
                    PageEvent::Load => state.loading = false,
                    PageEvent::Changed => {}
                }
            });
            false
        }
    }
}
fn request_step(params: &Value, main_frame: Option<&str>) -> Option<Value> {
    let id = params["requestId"].as_str()?;
    let url = params["request"]["url"]
        .as_str()
        .filter(|url| !url.starts_with("data:"))?;
    Some(
        json!({"kind":"request","id":id,"url":url.split('#').next().unwrap_or(url),"verb":params["request"]["method"].as_str().unwrap_or("GET"),"main":main_frame.is_some_and(|frame|params["frameId"]==frame),"navigation":params["loaderId"]==id&&params["type"]=="Document"}),
    )
}
struct Observer {
    target: String,
    desktop: bool,
    dialog: tokio::sync::watch::Sender<dialog::State>,
    main_frame: Option<String>,
    navigation: tokio::sync::watch::Sender<Navigation>,
    done: tokio::sync::watch::Sender<bool>,
    network: network::Kernel,
}
impl Observer {
    fn publish(&mut self, transition: network::Transition, clear: bool) {
        self.navigation.send_modify(|state| {
            state.revision = state.revision.wrapping_add(1);
            state.generation = self.network.generation;
            state.loading = self.network.loading;
            if let Some(report) = transition.reports.last() {
                state.report = Some(report.clone());
            } else if clear || transition.loaded_calls > 0 {
                state.report = None;
            }
            state.aborted = self.network.aborted.as_ref().map(|aborted| {
                state
                    .aborted
                    .as_ref()
                    .filter(|wait| wait.token == aborted.token)
                    .cloned()
                    .unwrap_or(AbortWait {
                        token: aborted.token,
                        expires: tokio::time::Instant::now() + Duration::from_secs(5),
                    })
            });
        });
    }
    fn accept(
        &mut self,
        value: &Value,
        session: &str,
        dialog_text: Option<&crate::cdp_dialog_json::DialogText>,
    ) -> bool {
        if value["sessionId"] == session {
            if value["method"] == "Page.javascriptDialogOpening" && self.main_frame.is_some() {
                let params = &value["params"];
                if let (Some(kind), Some(message)) =
                    (params["type"].as_str(), params["message"].as_str())
                {
                    self.dialog.send_modify(|state| {
                        let message = dialog_text
                            .and_then(|text| text.message.as_ref())
                            .cloned()
                            .unwrap_or_else(|| message.encode_utf16().collect());
                        let default = dialog_text
                            .and_then(|text| text.default_prompt.as_ref())
                            .cloned()
                            .unwrap_or_else(|| {
                                params["defaultPrompt"]
                                    .as_str()
                                    .unwrap_or("")
                                    .encode_utf16()
                                    .collect()
                            });
                        state.open_units(kind, &message, &default)
                    });
                }
            } else if self.desktop && value["method"] == "Page.javascriptDialogClosed" {
                self.dialog.send_modify(dialog::State::external_close);
            }
        }
        let event = match classify(
            value,
            Some(session),
            Some(&self.target),
            &mut self.main_frame,
        ) {
            Err(()) => return true,
            Ok(event) => event,
        };
        if value["sessionId"] == session
            && value["method"]
                .as_str()
                .is_some_and(|method| method.starts_with("Network."))
        {
            let params = &value["params"];
            let Some(id) = params["requestId"].as_str() else {
                return false;
            };
            let at = crate::store::iso(crate::store::now_ms());
            let before = self.network.aborted.as_ref().map(|value| value.token);
            let transition = match value["method"].as_str().unwrap() {
                "Network.requestWillBeSent" => {
                    let Some(step) = request_step(params, self.main_frame.as_deref()) else {
                        return false;
                    };
                    if self.network.has_request(id) && params["redirectResponse"].is_object() {
                        self.network.apply(&json!({"kind":"response","id":id,"status":params["redirectResponse"]["status"]}),&at);
                    }
                    self.network.replace_request(id);
                    self.network.apply(&step, &at)
                }
                "Network.responseReceived" if self.network.has_request(id) => self.network.apply(
                    &json!({"kind":"response","id":id,"status":params["response"]["status"]}),
                    &at,
                ),
                "Network.loadingFailed" if self.network.has_request(id) => {
                    let error = params["errorText"]
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .or_else(|| params["blockedReason"].as_str())
                        .unwrap_or("");
                    let result = self
                        .network
                        .apply(&json!({"kind":"failed","id":id,"errorText":error}), &at);
                    self.network.forget(id);
                    result
                }
                "Network.loadingFinished" => {
                    self.network.forget(id);
                    network::Transition::default()
                }
                _ => return false,
            };
            if !transition.reports.is_empty()
                || before != self.network.aborted.as_ref().map(|value| value.token)
            {
                self.publish(transition, false);
            }
            return false;
        }
        if let Some(event) = event {
            let url = value["params"]["frame"]["url"]
                .as_str()
                .or_else(|| value["params"]["url"].as_str())
                .or_else(|| value["params"]["targetInfo"]["url"].as_str());
            if let Some(url) = url {
                self.network.apply(&json!({"kind":"pageUrl","url":url}), "");
            }
            let (clear, transition) = match event {
                PageEvent::Start => {
                    self.network.loading = true;
                    (false, network::Transition::default())
                }
                PageEvent::Load => (true, self.network.apply(&json!({"kind":"load"}), "")),
                PageEvent::Changed => {
                    let clear = !self.network.loading;
                    (clear, network::Transition::default())
                }
            };
            self.publish(transition, clear);
        }
        false
    }
}
#[derive(Default)]
pub(super) struct Registry {
    controls: Vec<crate::preview_control::runtime::Control>,
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
            desktop: false,
            dialog: tokio::sync::watch::channel(dialog::State::default()).0,
            main_frame: None,
            navigation: nav_tx,
            done: done_tx,
            network: network::Kernel::new("about:blank".into()),
        },
    ) {
        old.done.send_replace(true);
    }
    (done, navigation)
}
pub(super) fn register_control(
    registry: &Shared,
    control: crate::preview_control::runtime::Control,
) {
    let mut registry = registry.lock().unwrap();
    registry.controls.retain(|control| !control.is_finished());
    registry.controls.push(control);
}
pub(super) fn control_receipts(registry: &Shared) -> Vec<crate::preview_control::runtime::Control> {
    registry.lock().unwrap().controls.clone()
}
pub(super) fn register_desktop(
    registry: &Shared,
    session: &str,
    target: &str,
) -> (
    tokio::sync::watch::Receiver<bool>,
    tokio::sync::watch::Receiver<Navigation>,
) {
    let watch = register(registry, session, target);
    registry
        .lock()
        .unwrap()
        .pages
        .get_mut(session)
        .unwrap()
        .desktop = true;
    watch
}
pub(super) fn frame(registry: &Shared, session: Option<&str>, result: &Value) {
    let mut registry = registry.lock().unwrap();
    if let Some(page) = session.and_then(|session| registry.pages.get_mut(session)) {
        page.main_frame = result["frameTree"]["frame"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        if let Some(url) = result["frameTree"]["frame"]["url"].as_str() {
            page.network.apply(&json!({"kind":"pageUrl","url":url}), "");
        }
    }
}
pub(super) fn remove(registry: &Shared, session: &str) {
    if let Some(page) = registry.lock().unwrap().pages.remove(session) {
        page.done.send_replace(true);
    }
}
#[cfg(test)]
pub(super) fn notify(registry: &Shared, value: &Value) -> bool {
    notify_packet(
        registry,
        &crate::cdp_dialog_json::Packet {
            value: value.clone(),
            dialog: None,
        },
    )
}
pub(super) fn notify_packet(registry: &Shared, packet: &crate::cdp_dialog_json::Packet) -> bool {
    let value = &packet.value;
    let mut registry = registry.lock().unwrap();
    let mut detached = Vec::new();
    for (session, page) in &mut registry.pages {
        if page.accept(value, session, packet.dialog.as_ref()) {
            page.done.send_replace(true);
            detached.push(session.clone());
        }
    }
    let removed = !detached.is_empty();
    for session in detached {
        registry.pages.remove(&session);
    }
    removed && registry.registered && registry.pages.is_empty()
}
pub(super) fn stopped(registry: &Shared) {
    let mut registry = registry.lock().unwrap();
    for page in registry.pages.values() {
        page.done.send_replace(true);
    }
    registry.pages.clear();
}
pub(super) fn entries(registry: &Shared, session: &str) -> Option<Vec<Value>> {
    registry
        .lock()
        .unwrap()
        .pages
        .get(session)
        .map(|page| page.network.entries())
}
pub(super) fn note_url(registry: &Shared, session: &str, url: &str) {
    if let Some(page) = registry.lock().unwrap().pages.get_mut(session) {
        page.network.apply(&json!({"kind":"pageUrl","url":url}), "");
    }
}
pub(super) fn expire(registry: &Shared, session: &str, token: u64) {
    if let Some(page) = registry.lock().unwrap().pages.get_mut(session) {
        if page
            .network
            .aborted
            .as_ref()
            .is_some_and(|aborted| aborted.token == token)
        {
            let transition = page.network.apply(&json!({"kind":"tick"}), "");
            page.publish(transition, false);
        }
    }
}
pub(super) fn dialog_state(registry: &Shared, session: &str) -> Option<dialog::State> {
    registry
        .lock()
        .unwrap()
        .pages
        .get(session)
        .map(|page| page.dialog.borrow().clone())
}
pub(super) fn dialog_watch(
    registry: &Shared,
    session: &str,
) -> Option<tokio::sync::watch::Receiver<dialog::State>> {
    registry
        .lock()
        .unwrap()
        .pages
        .get(session)
        .map(|page| page.dialog.subscribe())
}
pub(super) fn begin_dialog(
    registry: &Shared,
    session: &str,
    accept: bool,
    prompt: Option<&str>,
) -> Result<(u64, Value), &'static str> {
    let mut registry = registry.lock().unwrap();
    let page = registry
        .pages
        .get_mut(session)
        .ok_or("Browser tab is closed")?;
    let mut result = Err("No dialog is pending.");
    page.dialog
        .send_modify(|state| result = state.begin(accept, prompt));
    result
}
pub(super) fn finish_dialog(registry: &Shared, session: &str, token: u64) {
    if let Some(page) = registry.lock().unwrap().pages.get_mut(session) {
        page.dialog.send_modify(|state| state.finish(token));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_observer_removal_does_not_abort_the_following_close_command() {
        crate::test_sandbox::assert_isolated();
        let pages: Shared = Default::default();
        let (done, _) = register(&pages, "opaque", "target");
        remove(&pages, "opaque");
        assert!(*done.borrow());
        assert!(!notify(&pages, &json!({"id":19,"result":{}})));
    }
    #[test]
    fn frame_loading_preserves_the_native_main_request_report() {
        crate::test_sandbox::assert_isolated();
        let pages: Shared = Default::default();
        let (_, nav) = register(&pages, "opaque", "target");
        frame(
            &pages,
            Some("opaque"),
            &json!({"frameTree":{"frame":{"id":"frame"}}}),
        );
        notify(
            &pages,
            &json!({"method":"Network.requestWillBeSent","sessionId":"opaque","params":{"requestId":"request","loaderId":"request","type":"Document","frameId":"frame","request":{"url":"https://example.com/","method":"GET"}}}),
        );
        notify(
            &pages,
            &json!({"method":"Page.frameStartedLoading","sessionId":"opaque","params":{"frameId":"frame"}}),
        );
        assert_eq!(nav.borrow().report.as_ref().unwrap()["_tag"], "Loading");
        assert_eq!(nav.borrow().generation, 1);
    }
    #[test]
    fn pinned_upstream_request_classification_matches() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/preview-network-4288.json"
        ))
        .unwrap();
        let cases = fixture["adapterCases"].as_array().unwrap();
        assert_eq!(cases.len(), 192);
        for case in cases {
            assert_eq!(
                request_step(&case["input"], Some("main-frame")).unwrap_or(Value::Null),
                case["expected"]
            );
        }
    }
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
    #[test]
    fn dialogs_require_live_frame_and_exact_opaque_session_and_drop_on_detachment() {
        crate::test_sandbox::assert_isolated();
        let pages: Shared = Default::default();
        register_desktop(&pages, "opaque-a", "target-a");
        register(&pages, "opaque-b", "target-b");
        let opening = json!({"method":"Page.javascriptDialogOpening","sessionId":"opaque-a","params":{"type":"prompt","message":"Owned","defaultPrompt":"default"}});
        notify(&pages, &opening);
        assert!(dialog_state(&pages, "opaque-a").unwrap().status().is_null());
        frame(
            &pages,
            Some("opaque-a"),
            &json!({"frameTree":{"frame":{"id":"frame-a"}}}),
        );
        notify(&pages, &opening);
        assert_eq!(
            dialog_state(&pages, "opaque-a").unwrap().status()["message"],
            "Owned"
        );
        assert!(dialog_state(&pages, "opaque-b").unwrap().status().is_null());
        let revision = dialog_state(&pages, "opaque-a").unwrap().broadcast_revision;
        notify(
            &pages,
            &json!({"method":"Page.javascriptDialogClosed","sessionId":"opaque-b"}),
        );
        assert!(!dialog_state(&pages, "opaque-a").unwrap().status().is_null());
        notify(
            &pages,
            &json!({"method":"Page.javascriptDialogClosed","sessionId":"opaque-a"}),
        );
        assert!(dialog_state(&pages, "opaque-a").unwrap().status().is_null());
        assert_eq!(
            dialog_state(&pages, "opaque-a").unwrap().broadcast_revision,
            revision
        );
        notify(&pages, &opening);
        remove(&pages, "opaque-a");
        notify(&pages, &opening);
        assert!(dialog_state(&pages, "opaque-a").is_none());
        assert!(begin_dialog(&pages, "opaque-a", true, None).is_err());
    }
}
