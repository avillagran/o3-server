//! Bounded command correlation; one blocked page operation cannot block its dialog answer.
use super::*;
struct Call {
    method: String,
    session: Option<String>,
    expires: tokio::time::Instant,
    reply: oneshot::Sender<Result<Value, String>>,
}
#[derive(Default)]
pub(super) struct Calls {
    next: u64,
    calls: BTreeMap<u64, Call>,
}
impl Calls {
    pub fn full(&self) -> bool {
        self.calls.len() >= 8
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
        self.calls.values().map(|call| call.expires).min()
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
        fence
    }
}
impl Drop for Calls {
    fn drop(&mut self) {
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
}
