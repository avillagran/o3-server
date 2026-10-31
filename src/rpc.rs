//! Effect RPC over WebSocket, as spoken by T3 clients.
//! Frames: Request/Ack/Interrupt/Ping from the client; Chunk/Exit/Pong from us.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

pub const SERVER_VERSION: &str = concat!("o3-", env!("CARGO_PKG_VERSION"));

/// Unary handlers. `Err` carries the typed error placed in a `Fail` cause.
fn dispatch(tag: &str, _payload: &Value) -> Result<Value, Value> {
    Err(json!({ "_tag": "RpcNotImplemented", "message": format!("o3-server does not implement {tag}") }))
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
            let exit = match dispatch(frame["tag"].as_str()?, &frame["payload"]) {
                Ok(value) => json!({ "_tag": "Success", "value": value }),
                Err(error) => json!({ "_tag": "Failure", "cause": [{ "_tag": "Fail", "error": error }] }),
            };
            json!({ "_tag": "Exit", "requestId": frame["id"], "exit": exit })
        }
        _ => return None,
    };
    Some(Message::Text(reply.to_string().into()))
}
