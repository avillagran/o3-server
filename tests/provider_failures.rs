mod support;
use o3_server::provider::{Provider, ProviderControl, ProviderEvent, RunInput};
use serde_json::{json, Value};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

struct Failing {
    input: Value,
    before_start: bool,
}
impl Provider for Failing {
    fn execute<'a>(
        &'a self,
        input: RunInput,
        out: tokio::sync::mpsc::Sender<ProviderEvent>,
        _: tokio::sync::mpsc::Receiver<ProviderControl>,
        _: tokio::sync::watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            if !self.before_start {
                out.send(ProviderEvent::Codex(
                    json!({"method":"o3/binding","params":{"threadId":"native","cwd":input.cwd}}),
                ))
                .await
                .map_err(|e| e.to_string())?;
                out.send(ProviderEvent::Codex(json!({"method":"turn/started","params":{"threadId":"native","turn":{"id":"native-turn"}}}))).await.map_err(|e|e.to_string())?;
            }
            for _ in 0..2 {
                out.send(ProviderEvent::FailedWithDetails(self.input.clone()))
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }
}

fn failed(before_start: bool, input: Value) -> Value {
    let server = support::start(|state| {
        o3_server::settings::update(
            state,
            &json!({"patch":{"providerInstances":{"test":{"driver":"codex","enabled":true}}}}),
        )
        .unwrap();
        *state.provider.lock().unwrap() = Some(Arc::new(Failing {
            input,
            before_start,
        }));
    });
    let client = server.connect();
    client.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"t","threadId":"t","projectId":"p","title":"Failures","modelSelection":{"instanceId":"test","model":"test"},"runtimeMode":"full-access","interactionMode":"default"})).unwrap();
    client.rpc("orchestration.dispatchCommand",json!({"type":"message.dispatch","commandId":"m","threadId":"t","messageId":"m","text":"Private failure fixture","attachments":[],"dispatchMode":{"type":"start_immediately"}})).unwrap();
    let projection = || {
        client
            .rpc("orchestration.getThreadProjection", json!({"threadId":"t"}))
            .unwrap()
    };
    assert!(support::wait_for(Duration::from_secs(5), || projection()
        ["runs"][0]["status"]
        == "failed"));
    let p = projection();
    let cards = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "error")
        .collect::<Vec<_>>();
    assert_eq!(
        cards.len(),
        1,
        "duplicate terminal events must not duplicate cards"
    );
    assert_eq!(cards[0]["failure"]["message"], p["runs"][0]["error"]);
    assert!(p["visibleTurnItems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["item"]["id"] == cards[0]["id"]));
    let rows = t3_protocol::timeline::conversation(&p, &[], &str::to_owned);
    let errors = rows
        .iter()
        .filter(|row| row["role"] == "error")
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        1,
        "the actual client must render the terminal card"
    );
    assert_eq!(
        errors[0]["error"]["message"],
        cards[0]["failure"]["message"]
    );
    assert_eq!(errors[0]["error"]["class"], cards[0]["failure"]["class"]);
    assert_eq!(errors[0]["error"]["code"], cards[0]["failure"]["code"]);
    p
}

#[test]
fn native_terminal_failure_publishes_one_bounded_redacted_card() {
    let p = failed(
        false,
        json!({"class":"provider_error","message":"Request failed https://alice:private@example.com/path?secret=private#private Bearer private","code":"native-code","retryable":true}),
    );
    let card = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "error")
        .unwrap();
    assert_eq!(card["providerTurnId"], "codex-turn-native-turn");
    assert_eq!(
        card["id"],
        "turn-item:provider:codex:native-item:terminal-failure%3Acodex-turn-native-turn"
    );
    assert_eq!(card["title"], "Provider error");
    assert_eq!(
        card["failure"],
        json!({"class":"provider_error","message":"Request failed https://example.com/path Bearer [REDACTED]","code":"native-code","retryable":true})
    );
    assert_eq!(card["nodeId"], p["runs"][0]["rootNodeId"]);
}

#[test]
fn failed_start_uses_the_upstream_attempt_based_turn_identity() {
    let p = failed(
        true,
        json!({"class":"transport_error","cause":{"_tag":"ProviderAdapterOpenSessionError","message":"private raw defect"}}),
    );
    let card = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "error")
        .unwrap();
    assert!(card["providerTurnId"]
        .as_str()
        .unwrap()
        .starts_with("provider-turn:provider:codex:native-turn:failed%3A"));
    assert_eq!(card["failure"]["message"],"The provider session could not be opened. Check that the provider is installed and signed in, then retry the turn.");
    assert_eq!(card["failure"]["code"], Value::Null);
}

#[test]
fn usage_limit_card_preserves_class_retryability_and_reset_instant() {
    let p = failed(
        false,
        json!({"class":"usage_limit","message":"Limit reached","code":"quota","retryable":false,"resetAt":"2026-11-07T01:00:00+02:00"}),
    );
    let card = p["turnItems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "error")
        .unwrap();
    assert_eq!(card["title"], "Usage limit reached");
    assert_eq!(card["failure"]["resetAt"], "2026-11-06T23:00:00.000Z");
    assert_eq!(card["failure"]["retryable"], false);
    assert_eq!(p["runs"][0]["errorClass"], "usage_limit");
}
