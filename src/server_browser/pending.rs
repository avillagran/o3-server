//! Bounded command correlation; one blocked page operation cannot block its dialog answer.
use super::*;
struct Call {
    method: String,
    session: Option<String>,
    expires: tokio::time::Instant,
    reply: oneshot::Sender<Result<Value, String>>,
}
struct ExactCall {
    observer: Option<std::sync::Arc<dyn exact::Observer>>,
    permit: write::Permit,
    session: Option<String>,
    expires: tokio::time::Instant,
    reply: oneshot::Sender<Result<crate::native_page_worker::cdp::Packet, exact::Fault>>,
}
struct Late {
    session: Option<String>,
    observer: Option<std::sync::Arc<dyn exact::Observer>>,
}
#[derive(Default)]
pub(super) struct Calls {
    next: u64,
    calls: BTreeMap<u64, Call>,
    exact: BTreeMap<u64, ExactCall>,
    exact_late: BTreeMap<u64, Late>,
}
impl Calls {
    pub fn full(&self) -> bool {
        self.calls.len() + self.exact.len() >= 8
    }
    pub fn insert(&mut self, request: Request) -> Option<Value> {
        let Request {
            method,
            params,
            session,
            reply,
            expires,
        } = request;
        if reply.is_closed() || expires <= tokio::time::Instant::now() {
            let _ = reply.send(Err("Browser command timed out".into()));
            return None;
        }
        self.next += 1;
        let mut message = json!({"id":self.next,"method":method,"params":params});
        let session =
            session.filter(|_| !method.starts_with("Target.") && !method.starts_with("Browser."));
        if let Some(session) = &session {
            message["sessionId"] = json!(session);
        }
        self.calls.insert(
            self.next,
            Call {
                method,
                session,
                reply,
                expires: expires.min(tokio::time::Instant::now() + Duration::from_secs(5)),
            },
        );
        Some(message)
    }
    pub fn insert_exact(
        &mut self,
        request: exact::Request,
    ) -> Option<(u64, Vec<u8>, write::Permit)> {
        let exact::Request {
            method,
            observer,
            params,
            session,
            expires,
            reply,
            permit,
        } = request;
        if reply.is_closed() || expires <= tokio::time::Instant::now() {
            if let Some(observer) = &observer {
                observer.cancelled(false);
            }
            let _ = reply.send(Err(exact::Fault::Deadline));
            return None;
        }
        if self.exact_late.len() + self.exact.len() >= 64 {
            if let Some(observer) = &observer {
                observer.cancelled(false);
            }
            let _ = reply.send(Err(exact::Fault::Busy));
            return None;
        }
        if self.next >= 9_007_199_254_740_991 {
            if let Some(observer) = &observer {
                observer.cancelled(false);
            }
            let _ = reply.send(Err(exact::Fault::Protocol));
            return None;
        }
        self.next += 1;
        let bytes = match serde_json::to_vec(&exact::Command {
            id: self.next,
            method: &method,
            params: &params,
            session: session.as_deref(),
        }) {
            Ok(bytes) => bytes,
            Err(_) => {
                if let Some(observer) = &observer {
                    observer.cancelled(false);
                }
                let _ = reply.send(Err(exact::Fault::Protocol));
                return None;
            }
        };
        self.exact.insert(
            self.next,
            ExactCall {
                observer,
                permit: permit.clone(),
                session,
                expires,
                reply,
            },
        );
        Some((self.next, bytes, permit))
    }
    pub fn exact_unsent(&mut self, id: u64) {
        if let Some(call) = self.exact.remove(&id) {
            call.permit.cancel();
            if let Some(observer) = &call.observer {
                observer.cancelled(false);
            }
            let _ = call.reply.send(Err(exact::Fault::Busy));
        }
    }
    pub fn cancel_exact(&mut self) {
        let ids = self
            .exact
            .iter()
            .filter(|(_, call)| call.permit.cancelled() || call.reply.is_closed())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(call) = self.exact.remove(&id) {
                let started = call.permit.cancel();
                if let Some(observer) = &call.observer {
                    observer.cancelled(false);
                }
                if started {
                    self.exact_late.insert(
                        id,
                        Late {
                            session: call.session,
                            observer: call.observer.clone(),
                        },
                    );
                }
                let fault = if call.expires <= tokio::time::Instant::now() {
                    exact::Fault::Deadline
                } else {
                    exact::Fault::Closed
                };
                let _ = call.reply.send(Err(fault));
            }
        }
    }
    pub fn complete_exact(&mut self, bytes: &[u8]) -> bool {
        if self.exact.is_empty() && self.exact_late.is_empty() {
            return false;
        }
        let Ok(packet) = crate::native_page_worker::cdp::Packet::decode(bytes, 256 * 1024 * 1024)
        else {
            return false;
        };
        let Some(id) = packet.id() else {
            return false;
        };
        if let Some(late) = self.exact_late.get(&id) {
            if packet.session() == late.session.as_deref() {
                if let Some(observer) = &late.observer {
                    observer.completed(&packet);
                }
                self.exact_late.remove(&id);
            }
            return true;
        }
        let Some(call) = self.exact.get(&id) else {
            return false;
        };
        if packet.session() != call.session.as_deref() {
            return true;
        }
        let call = self.exact.remove(&id).unwrap();
        if let Some(observer) = &call.observer {
            observer.completed(&packet);
        }
        if call.expires <= tokio::time::Instant::now() {
            let _ = call.reply.send(Err(exact::Fault::Deadline));
            return true;
        }
        let valid = packet.method().is_none()
            && matches!(
                packet.payload(),
                Some(
                    crate::native_page_worker::cdp::Payload::Result(_)
                        | crate::native_page_worker::cdp::Payload::Error(_)
                )
            );
        let _ = call.reply.send(if valid {
            Ok(packet)
        } else {
            Err(exact::Fault::Protocol)
        });
        true
    }
    pub fn complete(&mut self, value: &Value, pages: &events::Shared) {
        let Some(id) = value["id"].as_u64() else {
            return;
        };
        let Some(call) = self.calls.get(&id) else {
            return;
        };
        if let (Some(expected), Some(actual)) =
            (call.session.as_deref(), value["sessionId"].as_str())
        {
            if expected != actual {
                return;
            }
        }
        let call = self.calls.remove(&id).unwrap();
        let result = if value.get("error").is_some() {
            Err("Browser rejected CDP command".into())
        } else {
            if call.method == "Page.getFrameTree" {
                events::frame(pages, call.session.as_deref(), &value["result"]);
            }
            Ok(value["result"].clone())
        };
        let _ = call.reply.send(result);
    }
    pub fn deadline(&self) -> Option<tokio::time::Instant> {
        self.calls
            .values()
            .map(|call| call.expires)
            .chain(self.exact.values().map(|call| call.expires))
            .min()
    }
    pub fn expire(&mut self, now: tokio::time::Instant) -> bool {
        let mut fence = false;
        let expired = self
            .calls
            .iter()
            .filter(|(_, call)| call.expires <= now)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(call) = self.calls.remove(&id) {
                fence |= call.method == "Target.createTarget";
                let _ = call.reply.send(Err("Browser command timed out".into()));
            }
        }
        let expired = self
            .exact
            .iter()
            .filter(|(_, call)| call.expires <= now)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(call) = self.exact.remove(&id) {
                let started = call.permit.cancel();
                if let Some(observer) = &call.observer {
                    observer.cancelled(started);
                }
                if started {
                    self.exact_late.insert(
                        id,
                        Late {
                            session: call.session,
                            observer: call.observer.clone(),
                        },
                    );
                }
                let _ = call.reply.send(Err(exact::Fault::Deadline));
            }
        }
        fence
    }
}
impl Drop for Calls {
    fn drop(&mut self) {
        for (_, call) in std::mem::take(&mut self.exact) {
            let started = call.permit.cancel();
            if let Some(observer) = &call.observer {
                observer.cancelled(started);
            }
            let _ = call.reply.send(Err(exact::Fault::Closed));
        }
        for (_, late) in std::mem::take(&mut self.exact_late) {
            if let Some(observer) = &late.observer {
                observer.cancelled(true);
            }
        }
        for (_, call) in std::mem::take(&mut self.calls) {
            let _ = call.reply.send(Err("Browser session ended".into()));
        }
    }
}
pub(super) async fn deadline(at: Option<tokio::time::Instant>) {
    if let Some(at) = at {
        tokio::time::sleep_until(at).await;
    } else {
        std::future::pending::<()>().await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn request(method: &str) -> (Request, oneshot::Receiver<Result<Value, String>>) {
        let (reply, receive) = oneshot::channel();
        (
            Request {
                method: method.into(),
                params: json!({}),
                session: Some("opaque".into()),
                reply,
                expires: tokio::time::Instant::now() + Duration::from_secs(6),
            },
            receive,
        )
    }
    struct ExactLease {
        _done: tokio::sync::watch::Sender<bool>,
        _control: crate::preview_control::runtime::Control,
    }
    fn exact_request(
        expires: tokio::time::Instant,
    ) -> (
        exact::Request,
        oneshot::Receiver<Result<crate::native_page_worker::cdp::Packet, exact::Fault>>,
        ExactLease,
    ) {
        let (reply, received) = oneshot::channel();
        let (done_owner, done) = tokio::sync::watch::channel(false);
        let control = crate::preview_control::runtime::Control::new(None);
        let permit = write::Permit::new(expires, done, control.subscribe(), Default::default());
        (
            exact::Request {
                method: "Runtime.evaluate".into(),
                observer: None,
                permit,
                params: exact::parameters(br#"{"expression":"'\ud800'","returnByValue":true}"#)
                    .unwrap(),
                session: Some("exact-session".into()),
                expires,
                reply,
            },
            received,
            ExactLease {
                _done: done_owner,
                _control: control,
            },
        )
    }
    #[tokio::test]
    async fn exact_and_scalar_responses_keep_unique_ids_and_captured_sessions() {
        crate::test_sandbox::assert_isolated();
        let pages = events::Shared::default();
        let mut calls = Calls::default();
        let (raw_request, mut exact_reply, _lease) =
            exact_request(tokio::time::Instant::now() + Duration::from_secs(2));
        let (_, raw, _) = calls.insert_exact(raw_request).unwrap();
        assert!(serde_json::from_slice::<Value>(&raw).is_err());
        let raw_id = crate::native_page_worker::cdp::Packet::decode(&raw, 4096)
            .unwrap()
            .id()
            .unwrap();
        let (scalar, scalar_reply) = request("Runtime.evaluate");
        let scalar_id = calls.insert(scalar).unwrap()["id"].as_u64().unwrap();
        assert_ne!(raw_id, scalar_id);
        assert!(calls.complete_exact(
            format!(r#"{{"id":{raw_id},"sessionId":"foreign","result":{{"value":"\udfff"}}}}"#)
                .as_bytes()
        ));
        assert!(matches!(
            exact_reply.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        calls.complete(
            &json!({"id":scalar_id,"sessionId":"opaque","result":{"value":"scalar survives"}}),
            &pages,
        );
        assert_eq!(
            scalar_reply.await.unwrap().unwrap()["value"],
            "scalar survives"
        );
        assert!(calls.complete_exact(
            format!(
                r#"{{"id":{raw_id},"sessionId":"exact-session","result":{{"value":"\udfff"}}}}"#
            )
            .as_bytes()
        ));
        let packet = exact_reply.await.unwrap().unwrap();
        let Some(crate::native_page_worker::cdp::Payload::Result(value)) = packet.payload() else {
            panic!("Expected exact result")
        };
        assert_eq!(value.json(), r#"{"value":"\udfff"}"#);
    }
    #[tokio::test]
    async fn expired_exact_results_are_drained_and_capacity_is_not_reclaimed_by_foreign_sessions() {
        crate::test_sandbox::assert_isolated();
        let mut calls = Calls::default();
        let now = tokio::time::Instant::now();
        for _ in 0..64 {
            let (request, received, _lease) = exact_request(now + Duration::from_secs(60));
            let (_, _, permit) = calls.insert_exact(request).unwrap();
            assert!(permit.begin());
            assert!(!calls.expire(now + Duration::from_secs(61)));
            assert_eq!(received.await.unwrap().unwrap_err(), exact::Fault::Deadline);
        }
        let (request, received, _lease) = exact_request(now + Duration::from_secs(60));
        assert!(calls.insert_exact(request).is_none());
        assert_eq!(received.await.unwrap().unwrap_err(), exact::Fault::Busy);
        assert!(
            calls.complete_exact(br#"{"id":1,"sessionId":"foreign","result":{"value":"\ud800"}}"#)
        );
        assert_eq!(calls.exact_late.len(), 64);
        assert!(calls.complete_exact(
            br#"{"id":1,"sessionId":"exact-session","result":{"value":"\ud800"}}"#
        ));
        assert_eq!(calls.exact_late.len(), 63);
        let (request, received, _lease) = exact_request(now + Duration::from_secs(60));
        assert!(calls.insert_exact(request).is_some());
        drop(calls);
        assert_eq!(received.await.unwrap().unwrap_err(), exact::Fault::Closed);
    }
    #[tokio::test]
    async fn out_of_order_dialog_answer_and_error_preserve_other_pending_calls() {
        crate::test_sandbox::assert_isolated();
        let pages = events::Shared::default();
        let mut calls = Calls::default();
        let (evaluate, eval) = request("Runtime.evaluate");
        let eval_id = calls.insert(evaluate).unwrap()["id"].clone();
        let (answer, answered) = request("Page.handleJavaScriptDialog");
        let answer_id = calls.insert(answer).unwrap()["id"].clone();
        calls.complete(
            &json!({"id":eval_id,"sessionId":"wrong","result":{"ignored":true}}),
            &pages,
        );
        calls.complete(
            &json!({"id":answer_id,"sessionId":"opaque","result":{}}),
            &pages,
        );
        assert_eq!(answered.await.unwrap().unwrap(), json!({}));
        calls.complete(
            &json!({"id":eval_id,"error":{"message":"Synthetic rejection"}}),
            &pages,
        );
        assert_eq!(
            eval.await.unwrap().unwrap_err(),
            "Browser rejected CDP command"
        );
        let (pending, expired) = request("Runtime.evaluate");
        assert!(calls.insert(pending).is_some());
        let end = calls.deadline().unwrap();
        assert!(!calls.expire(end));
        assert_eq!(
            expired.await.unwrap().unwrap_err(),
            "Browser command timed out"
        );
        assert!(calls.deadline().is_none());
        let (next, receiver) = request("Page.getNavigationHistory");
        let id = calls.insert(next).unwrap()["id"].clone();
        calls.complete(&json!({"id":id,"result":{"alive":true}}), &pages);
        assert_eq!(receiver.await.unwrap().unwrap(), json!({"alive":true}));
    }
    #[tokio::test]
    async fn deadlines_are_independent_and_unowned_late_creation_is_fenced() {
        crate::test_sandbox::assert_isolated();
        let pages = events::Shared::default();
        let mut calls = Calls::default();
        let (mut first, expired) = request("Runtime.evaluate");
        first.expires = tokio::time::Instant::now() + Duration::from_secs(1);
        calls.insert(first).unwrap();
        let first_deadline = calls.deadline().unwrap();
        let (second, mut live) = request("Page.getNavigationHistory");
        let second_id = calls.insert(second).unwrap()["id"].clone();
        assert!(!calls.expire(first_deadline));
        assert!(expired.await.unwrap().is_err());
        assert!(matches!(
            live.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        calls.complete(&json!({"id":second_id,"result":{"live":true}}), &pages);
        assert_eq!(live.await.unwrap().unwrap(), json!({"live":true}));
        let (mut stale, rejected) = request("Target.createTarget");
        stale.expires = tokio::time::Instant::now();
        assert!(calls.insert(stale).is_none());
        assert!(rejected.await.unwrap().is_err());
        let (create, timed_out) = request("Target.createTarget");
        calls.insert(create).unwrap();
        assert!(calls.expire(calls.deadline().unwrap()));
        assert!(timed_out.await.unwrap().is_err());
    }
    #[derive(Default)]
    struct RecordObserver {
        completed: std::sync::Mutex<Vec<crate::native_page_worker::cdp::Packet>>,
        cancelled: std::sync::Mutex<Vec<bool>>,
    }
    impl exact::Observer for RecordObserver {
        fn completed(&self, packet: &crate::native_page_worker::cdp::Packet) {
            self.completed.lock().unwrap().push(packet.clone());
        }
        fn cancelled(&self, unknown: bool) {
            self.cancelled.lock().unwrap().push(unknown);
        }
    }
    #[tokio::test]
    async fn late_root_attachment_retains_its_completion_hook_after_caller_close() {
        crate::test_sandbox::assert_isolated();
        let (mut request, received, _lease) =
            exact_request(tokio::time::Instant::now() + Duration::from_secs(3));
        let observer = std::sync::Arc::new(RecordObserver::default());
        request.method = "Target.attachToTarget".into();
        request.session = None;
        request.observer = Some(observer.clone());
        request.params = crate::native_page_worker::JavaScriptJson::parse(
            r#"{"targetId":"owned","flatten":true}"#.into(),
        )
        .unwrap();
        let mut calls = Calls::default();
        let (id, bytes, permit) = calls.insert_exact(request).unwrap();
        assert_eq!(id, 1);
        assert!(permit.begin());
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["method"], "Target.attachToTarget");
        assert!(value.get("sessionId").is_none());
        drop(received);
        calls.cancel_exact();
        assert_eq!(*observer.cancelled.lock().unwrap(), vec![false]);
        assert_eq!(calls.exact_late.len(), 1);
        assert!(calls.complete_exact(
            br#"{"id":1,"sessionId":"foreign","result":{"sessionId":"must-not-register"}}"#
        ));
        assert!(observer.completed.lock().unwrap().is_empty());
        assert!(calls.complete_exact(
            br#"{"id":1,"result":{"sessionId":"late-owned","metadata":"\ud800"}}"#
        ));
        let records = observer.completed.lock().unwrap();
        assert_eq!(records.len(), 1);
        let crate::native_page_worker::cdp::Payload::Result(raw) = records[0].payload().unwrap()
        else {
            panic!("result missing")
        };
        assert!(raw.json().contains("late-owned"));
        assert!(raw.json().contains("\\ud800"));
        assert!(calls.exact_late.is_empty());
    }
    #[tokio::test]
    async fn losing_the_actor_marks_retained_late_completion_as_unknown() {
        crate::test_sandbox::assert_isolated();
        let (mut request, received, _lease) =
            exact_request(tokio::time::Instant::now() + Duration::from_secs(3));
        let observer = std::sync::Arc::new(RecordObserver::default());
        request.observer = Some(observer.clone());
        let mut calls = Calls::default();
        let (_, _, permit) = calls.insert_exact(request).unwrap();
        assert!(permit.begin());
        drop(received);
        calls.cancel_exact();
        drop(calls);
        assert_eq!(*observer.cancelled.lock().unwrap(), vec![false, true]);
        assert!(observer.completed.lock().unwrap().is_empty());
    }
}
