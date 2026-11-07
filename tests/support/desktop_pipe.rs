//! Owned desktop-channel peer. No browser process, profile deletion or host display.
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn install(
    state: Arc<o3_server::store::State>,
) -> (
    o3_server::preview::DesktopDriver,
    tokio::task::JoinHandle<()>,
    Arc<Mutex<Vec<Value>>>,
) {
    let (mut launcher, input) = tokio::io::duplex(65536);
    let (control, commands) = tokio::io::duplex(65536);
    let channel = o3_server::desktop_browser_channel::Channel::start(input, control);
    let driver = o3_server::preview::install_desktop(state.clone(), channel).await;
    let messages = Arc::new(Mutex::new(Vec::new()));
    let observed = messages.clone();
    let task = tokio::spawn(async move {
        let mut commands = BufReader::new(commands).lines();
        let (events, mut incoming) = tokio::sync::mpsc::unbounded_channel();
        struct Discovery(tokio::task::JoinHandle<()>);
        impl Drop for Discovery {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _discovery = Discovery(tokio::spawn(async move {
            let mut attached = std::collections::HashSet::new();
            let mut discovery = tokio::time::interval(std::time::Duration::from_millis(5));
            loop {
                discovery.tick().await;
                let list =
                    o3_server::preview::call(&state, "preview.list", &json!({"threadId":"t"}))
                        .await
                        .unwrap();
                for session in list["sessions"].as_array().unwrap() {
                    let tab = session["tabId"].as_str().unwrap();
                    if attached.insert(tab.to_owned())
                        && events
                            .send(json!({"type":"attached","threadId":"t","tabId":tab}))
                            .is_err()
                    {
                        return;
                    }
                }
            }
        }));
        loop {
            let events = tokio::select! {
                event = incoming.recv() => match event {Some(event)=>vec![event],None=>break},
                line = commands.next_line() => {
                    let Ok(Some(line)) = line else { break; };
                    let command: Value = serde_json::from_str(&line).unwrap();
                    observed.lock().unwrap().push(command.clone());
                    if command["type"] != "cdp" { continue; }
                    let packet: Value = serde_json::from_str(command["message"].as_str().unwrap()).unwrap();
                    let result = match packet["method"].as_str().unwrap() {
                        "Browser.getVersion" => json!({"userAgent":"Mozilla/5.0 (X11; Linux x86_64) Chrome/144.0.0.0"}),
                        "Target.getTargets" => json!({"targetInfos":[{"type":"page","targetId":"owned-page"}]}),
                        "Target.attachToTarget" => json!({"sessionId":"owned-session"}),
                        "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"main"}}}),
                        "Page.getNavigationHistory" => json!({"currentIndex":0,"entries":[{"url":"http://localhost:1234/next","title":"Owned fixture"}]}),
                        "Runtime.evaluate" => json!({"result":{"value":{"url":"http://localhost:1234/next","title":"Owned fixture","text":"Fixture body","readyState":"complete"}}}),
                        _ => json!({}),
                    };
                    vec![json!({"type":"cdp","threadId":command["threadId"],"tabId":command["tabId"],"message":json!({"id":packet["id"],"result":result}).to_string()})]
                }
            };
            for event in events {
                if launcher
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    });
    (driver, task, messages)
}
