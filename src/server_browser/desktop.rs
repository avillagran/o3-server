//! Frontend-owned page adoption through a private channel-owned loopback CDP relay.
use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

fn detached(value: &Value, session: Option<&str>) -> bool {
    value["method"] == "Inspector.detached"
        || (value["method"] == "Target.detachedFromTarget"
            && session.is_some_and(|session| value["params"]["sessionId"] == session))
}
pub(super) async fn launch(
    channel: &crate::desktop_browser_channel::Channel,
    thread: &str,
    tab: &str,
    profile: &str,
) -> Result<Tab, String> {
    let mut changes = channel.subscribe();
    let endpoint = channel
        .endpoint(crate::desktop_browser::TabKey {
            thread_id: thread.into(),
            tab_id: tab.into(),
        })
        .await
        .map_err(|_| "Cannot create desktop browser connection")?;
    // This URL is owned by Channel, never supplied by an RPC caller or written to logs.
    let url = url::Url::parse(endpoint.url()).map_err(|_| "Invalid desktop browser connection")?;
    if url.scheme() != "ws" || url.host_str() != Some("127.0.0.1") {
        return Err("Invalid desktop browser connection".into());
    }
    let stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect((
            "127.0.0.1",
            url.port().ok_or("Invalid desktop browser connection")?,
        )),
    )
    .await
    .map_err(|_| "Desktop browser connection timed out")?
    .map_err(|_| "Desktop browser connection failed")?;
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::client_async_with_config(
            endpoint.url(),
            stream,
            Some(
                tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                    .max_message_size(Some(4 * 1024 * 1024))
                    .max_frame_size(Some(4 * 1024 * 1024)),
            ),
        ),
    )
    .await
    .map_err(|_| "Desktop browser connection timed out")?
    .map_err(|_| "Desktop browser connection failed")?;
    let (tx, mut rx) = mpsc::channel::<Request>(8);
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    let stop_signal = cancel.clone();
    let thread_owned = thread.to_string();
    let tab_owned = tab.to_string();
    tokio::spawn(async move {
        let mut id = 0_u64;
        let mut attached_session: Option<String> = None;
        loop {
            let request = tokio::select! {
                request=rx.recv()=>request,
                change=changes.next()=>{match change {Some(change) if change.key.thread_id==thread_owned && change.key.tab_id==tab_owned && !change.attached=>break,Some(_)=>continue,None=>break}},
                _=stop_signal.notified()=>None,
                incoming=socket.next()=>{
                    match incoming{Some(Ok(Message::Text(text)))=>{if serde_json::from_str::<Value>(&text).is_ok_and(|value|detached(&value,attached_session.as_deref())){break;}continue;},Some(Ok(Message::Ping(payload)))=>{if socket.send(Message::Pong(payload)).await.is_err(){break;}continue;},Some(Ok(Message::Pong(_)))=>continue,_=>break}
                }
            };
            let Some((method, params, session, reply)) = request else {
                break;
            };
            id += 1;
            let work = async {
                let mut message = json!({"id":id,"method":method,"params":params});
                if !method.starts_with("Target.") && !method.starts_with("Browser.") {
                    if let Some(session) = session {
                        message["sessionId"] = json!(session);
                    }
                }
                socket
                    .send(Message::Text(message.to_string().into()))
                    .await
                    .map_err(|_| "CDP write failed".to_string())?;
                loop {
                    match socket.next().await {
                        Some(Ok(Message::Text(text))) => {
                            let value: Value = serde_json::from_str(&text)
                                .map_err(|_| "Invalid CDP output".to_string())?;
                            if detached(&value, attached_session.as_deref()) {
                                return Err("Desktop browser disconnected".into());
                            }
                            if value["id"] == id {
                                if value.get("error").is_some() {
                                    return Err("Browser rejected CDP command".into());
                                }
                                if method == "Target.attachToTarget" {
                                    attached_session =
                                        value["result"]["sessionId"].as_str().map(str::to_owned);
                                }
                                return Ok(value["result"].clone());
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => socket
                            .send(Message::Pong(payload))
                            .await
                            .map_err(|_| "CDP write failed".to_string())?,
                        Some(Ok(Message::Pong(_))) => {}
                        _ => return Err("Desktop browser disconnected".into()),
                    }
                }
            };
            let result = tokio::select! { _=async {loop {match changes.next().await {Some(change) if change.key.thread_id==thread_owned && change.key.tab_id==tab_owned && !change.attached=>break,Some(_)=>{},None=>break}}}=>Err("Desktop browser disconnected".into()),_=stop_signal.notified()=>Err("Desktop browser disconnected".into()),result=tokio::time::timeout(Duration::from_secs(5),work)=>result.unwrap_or_else(|_|Err("Browser command timed out".into()))};
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed {
                break;
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
        endpoint.close().await;
        let _ = done_tx.send(true);
    });
    let mut browser = Tab {
        session: None,
        target: None,
        profile: profile.into(),
        origin: Origin::Desktop,
        tx,
        done,
        cancel,
    };
    let initialized = async {
        let targets = request(&browser.tx, "Target.getTargets", json!({})).await?;
        let target = targets["targetInfos"]
            .as_array()
            .and_then(|targets| targets.iter().find(|target| target["type"] == "page"))
            .and_then(|target| target["targetId"].as_str())
            .ok_or("The desktop tab has no page.")?
            .to_owned();
        let attached = request(
            &browser.tx,
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
        )
        .await?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or("Desktop page attachment failed")?
            .to_owned();
        request_in(&browser.tx, "Page.enable", json!({}), Some(session.clone())).await?;
        browser.target = Some(target);
        browser.session = Some(session);
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = initialized {
        stop(browser).await;
        return Err(error);
    }
    Ok(browser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as SyncMutex};
    struct Peer {
        channel: crate::desktop_browser_channel::Channel,
        events: mpsc::UnboundedSender<Value>,
        messages: Arc<SyncMutex<Vec<Value>>>,
        task: tokio::task::JoinHandle<()>,
    }
    fn key() -> crate::desktop_browser::TabKey {
        crate::desktop_browser::TabKey {
            thread_id: "thread".into(),
            tab_id: "tab".into(),
        }
    }
    async fn setup(page: bool) -> Peer {
        crate::test_sandbox::assert_isolated();
        let (mut launcher, input) = tokio::io::duplex(65536);
        let (control, commands) = tokio::io::duplex(65536);
        let channel = crate::desktop_browser_channel::Channel::start(input, control);
        let (events, mut incoming) = mpsc::unbounded_channel::<Value>();
        let messages = Arc::new(SyncMutex::new(Vec::new()));
        let captured = messages.clone();
        let task = tokio::spawn(async move {
            let mut commands = BufReader::new(commands);
            loop {
                let mut line = String::new();
                let event = tokio::select! {event=incoming.recv()=>{let Some(event)=event else{break;};event},read=commands.read_line(&mut line)=>{if !read.is_ok_and(|n|n>0){break;}let command:Value=serde_json::from_str(&line).unwrap();captured.lock().unwrap().push(command.clone());if command["type"]!="cdp"{continue;}let packet:Value=serde_json::from_str(command["message"].as_str().unwrap()).unwrap();let unrelated=json!({"type":"cdp","threadId":"thread","tabId":"tab","message":json!({"id":0,"result":{"unrelated":true}}).to_string()});launcher.write_all(format!("{unrelated}\n").as_bytes()).await.unwrap();let result=match string(&packet,"method"){"Target.getTargets"=>json!({"targetInfos":if page{json!([{"type":"worker","targetId":"worker"},{"type":"page","targetId":"first-page"},{"type":"page","targetId":"second-page"}])}else{json!([])}}),"Target.attachToTarget"=>json!({"sessionId":"opaque/session 😀"}),_=>json!({"accepted":true})};json!({"type":"cdp","threadId":"thread","tabId":"tab","message":json!({"id":packet["id"],"result":result}).to_string()})}};
                launcher
                    .write_all(format!("{event}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        events
            .send(json!({"type":"attached","threadId":"thread","tabId":"tab"}))
            .unwrap();
        assert!(channel.await_attached(&key(), Duration::from_secs(1)).await);
        Peer {
            channel,
            events,
            messages,
            task,
        }
    }
    fn string<'a>(value: &'a Value, name: &str) -> &'a str {
        value[name].as_str().unwrap_or("")
    }
    fn packets(peer: &Peer) -> Vec<Value> {
        peer.messages
            .lock()
            .unwrap()
            .iter()
            .filter(|message| message["type"] == "cdp")
            .map(|message| serde_json::from_str(message["message"].as_str().unwrap()).unwrap())
            .collect()
    }
    async fn stopped(browser: &Browsers) {
        let mut done = browser.completion("thread", "tab").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !*done.borrow() {
                done.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn adopts_first_existing_page_and_routes_sessions_without_navigation_or_page_destruction()
    {
        let peer = setup(true).await;
        let browser = Browsers::default();
        browser
            .open_desktop(&peer.channel, "thread", "tab", "default")
            .await
            .unwrap();
        assert_eq!(
            browser.connection("thread", "tab").await,
            Some((Origin::Desktop, true))
        );
        let initial = packets(&peer);
        assert_eq!(
            initial
                .iter()
                .map(|packet| string(packet, "method"))
                .collect::<Vec<_>>(),
            vec!["Target.getTargets", "Target.attachToTarget", "Page.enable"]
        );
        assert_eq!(
            initial[1]["params"],
            json!({"targetId":"first-page","flatten":true})
        );
        assert_eq!(initial[2]["sessionId"], "opaque/session 😀");
        assert_eq!(
            browser
                .command(
                    "thread",
                    "tab",
                    "Runtime.evaluate",
                    json!({"expression":"1"})
                )
                .await
                .unwrap(),
            json!({"accepted":true})
        );
        let last = packets(&peer).pop().unwrap();
        assert_eq!(last["sessionId"], "opaque/session 😀");
        browser.close("thread", Some("tab")).await;
        assert!(peer
            .messages
            .lock()
            .unwrap()
            .iter()
            .any(|message| message["type"] == "release"));
        assert!(!packets(&peer).iter().any(|packet| matches!(
            string(packet, "method"),
            "Target.createTarget" | "Target.closeTarget" | "Browser.close" | "Page.navigate"
        )));
        assert_eq!(browser.connection("thread", "tab").await, None);
        peer.task.abort();
    }
    #[tokio::test]
    async fn desktop_detachment_retains_origin_and_reconnects_with_fresh_endpoint() {
        let peer = setup(true).await;
        let browser = Browsers::default();
        browser
            .open_desktop(&peer.channel, "thread", "tab", "default")
            .await
            .unwrap();
        peer.events
            .send(json!({"type":"detached","threadId":"thread","tabId":"tab"}))
            .unwrap();
        stopped(&browser).await;
        assert_eq!(
            browser.connection("thread", "tab").await,
            Some((Origin::Desktop, false))
        );
        assert!(browser
            .command("thread", "tab", "Runtime.evaluate", json!({}))
            .await
            .is_err());
        peer.events
            .send(json!({"type":"attached","threadId":"thread","tabId":"tab"}))
            .unwrap();
        assert!(
            peer.channel
                .await_attached(&key(), Duration::from_secs(1))
                .await
        );
        browser
            .open_desktop(&peer.channel, "thread", "tab", "default")
            .await
            .unwrap();
        assert_eq!(
            browser.connection("thread", "tab").await,
            Some((Origin::Desktop, true))
        );
        assert_eq!(
            packets(&peer)
                .iter()
                .filter(|packet| packet["method"] == "Target.getTargets")
                .count(),
            2
        );
        browser.shutdown().await;
        peer.task.abort();
    }
    #[tokio::test]
    async fn missing_desktop_page_releases_connection_without_creating_a_page() {
        let peer = setup(false).await;
        let browser = Browsers::default();
        let error = browser
            .open_desktop(&peer.channel, "thread", "tab", "default")
            .await
            .unwrap_err();
        assert_eq!(error["message"], "The desktop tab has no page.");
        assert_eq!(browser.connection("thread", "tab").await, None);
        assert_eq!(packets(&peer).len(), 1);
        assert!(peer
            .messages
            .lock()
            .unwrap()
            .iter()
            .any(|message| message["type"] == "release"));
        peer.task.abort();
    }
}
