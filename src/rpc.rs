//! Effect RPC over WebSocket, as spoken by T3 clients.
//! Frames: Request/Ack/Interrupt/Ping from the client; Chunk/Exit/Pong from us.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

pub const SERVER_VERSION: &str = concat!("o3-", env!("CARGO_PKG_VERSION"));
const SHELL_SCHEMA_VERSION: u64 = 1;

/// Unary handlers. `Err` carries the typed error placed in a `Fail` cause.
fn dispatch(tag: &str, _payload: &Value) -> Result<Value, Value> {
    Err(json!({ "_tag": "RpcNotImplemented", "message": format!("o3-server does not implement {tag}") }))
}

/// Items a subscription starts with. Open streams stay open until `Interrupt` or disconnect.
fn stream_open(tag: &str, _payload: &Value) -> Option<Vec<Value>> {
    match tag {
        "orchestration.subscribeShell" => Some(vec![json!({ "kind": "snapshot", "snapshot": {
            "schemaVersion": SHELL_SCHEMA_VERSION, "snapshotSequence": 0,
            "projects": [], "threads": [], "archivedThreads": []
        }})]),
        _ => None,
    }
}

fn chunk(id: &Value, values: Vec<Value>) -> Message {
    Message::Text(json!({ "_tag": "Chunk", "requestId": id, "values": values }).to_string().into())
}

pub async fn run<S: AsyncRead + AsyncWrite + Unpin>(mut ws: WebSocketStream<S>) {
    while let Some(Ok(message)) = ws.next().await {
        let reply = match message {
            Message::Text(text) => handle(&text),
            Message::Ping(data) => Some(Message::Pong(data)),
            Message::Close(_) => break,
            _ => None,
        };
        if let Some(reply) = reply {
            if ws.send(reply).await.is_err() {
                break;
            }
        }
    }
}

fn handle(text: &str) -> Option<Message> {
    let frame: Value = serde_json::from_str(text).ok()?;
    let reply = match frame["_tag"].as_str()? {
        "Ping" => json!({ "_tag": "Pong" }),
        "Request" => {
            let tag = frame["tag"].as_str()?;
            if let Some(items) = stream_open(tag, &frame["payload"]) {
                return Some(chunk(&frame["id"], items));
            }
            let exit = match dispatch(tag, &frame["payload"]) {
                Ok(value) => json!({ "_tag": "Success", "value": value }),
                Err(error) => json!({ "_tag": "Failure", "cause": [{ "_tag": "Fail", "error": error }] }),
            };
            json!({ "_tag": "Exit", "requestId": frame["id"], "exit": exit })
        }
        _ => return None,
    };
    Some(Message::Text(reply.to_string().into()))
}
