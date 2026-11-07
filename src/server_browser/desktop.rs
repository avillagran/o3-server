//! Frontend-owned page adoption through a private channel-owned loopback CDP relay.
use super::*;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;
// Playwright 1.60.0 WebSocketTransport uses this receive-byte bound.
const MAX_REPLY_BYTES: usize = 256 * 1024 * 1024;

pub(super) async fn launch(
    channel: &crate::desktop_browser_channel::Channel,
    thread: &str,
    tab: &str,
    profile: &str,
    owner: Option<&str>,
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
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::client_async_with_config(
            endpoint.url(),
            stream,
            Some(
                tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                    .max_message_size(Some(MAX_REPLY_BYTES))
                    .max_frame_size(Some(MAX_REPLY_BYTES)),
            ),
        ),
    )
    .await
    .map_err(|_| "Desktop browser connection timed out")?
    .map_err(|_| "Desktop browser connection failed")?;
    let (output, mut input) = socket.split();
    let mut writer = write::websocket(output);
    let (tx, mut rx) = mpsc::channel::<Request>(8);
    let (exact_tx, mut exact_rx) = mpsc::channel::<exact::Request>(8);
    let exact_wake = std::sync::Arc::new(tokio::sync::Notify::new());
    let write_wake = exact_wake.clone();
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let pages: events::Shared = Default::default();
    let actor_pages = pages.clone();
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    let stop_signal = cancel.clone();
    let thread_owned = thread.to_string();
    let tab_owned = tab.to_string();
    tokio::spawn(async move {
        let mut calls = pending::Calls::default();
        let mut writing_done = writer.completion();
        loop {
            let expires = calls.deadline();
            tokio::select! {
                request=rx.recv(),if !calls.legacy_full()=>{
                    let Some(request)=request else{break;};
                    let Some(message)=calls.insert(request)else{continue;};
                    if !writer.send(Message::Text(message.to_string().into())){break;}
                },
                next=exact_rx.recv()=>{
                    let Some(next)=next else{break;};
                    let Some((id,bytes,permit))=calls.insert_exact(next)else{continue;};
                    let Ok(text)=String::from_utf8(bytes)else{break;};
                    if !writer.send_exact(Message::Text(text.into()),permit){calls.exact_unsent(id);}
                },
                _=write_wake.notified()=>calls.cancel_exact(),
                _=pending::deadline(expires)=>{if calls.expire(tokio::time::Instant::now()){break;}},
                _=writing_done.changed()=>break,
                change=changes.next()=>{match change{Some(change)if change.key.thread_id==thread_owned&&change.key.tab_id==tab_owned&&!change.attached=>break,Some(_)=>{},None=>break}},
                _=stop_signal.notified()=>break,
                incoming=input.next()=>{
                    match incoming{
                        Some(Ok(Message::Text(text)))=>{
                            if calls.complete_exact(text.as_bytes()){continue;}
                            if native_owner::route(&actor_pages,text.as_bytes()){continue;}
                            let Ok(packet)=crate::cdp_dialog_json::decode(text.as_bytes())else{break;};
                            if events::notify_packet(&actor_pages,&packet){break;}
                            calls.complete(&packet.value,&actor_pages);
                        },
                        Some(Ok(Message::Ping(payload)))=>{if !writer.send(Message::Pong(payload)){break;}},
                        Some(Ok(Message::Pong(_)))=>{},
                        _=>break
                    }
                }
            }
        }
        drop(rx);
        drop(exact_rx);
        drop(calls);
        native_owner::stopped(&actor_pages);
        let controls = events::control_receipts(&actor_pages);
        for control in &controls {
            drop(control.close());
        }
        writer.stop().await;
        drop(input);
        endpoint.close().await;
        events::stopped(&actor_pages);
        let mut cleanup_ready = true;
        for control in &controls {
            if control.shutdown().await.is_err() {
                cleanup_ready = false;
            }
        }
        if cleanup_ready {
            let _ = done_tx.send(true);
        }
    });
    let mut browser = Tab {
        exact_tx,
        exact_wake,
        keyboard: Default::default(),
        closing: false,
        activity: Default::default(),
        control: Some(crate::preview_control::runtime::Control::new(
            owner.map(str::to_owned),
        )),
        session: None,
        target: None,
        profile: profile.into(),
        origin: Origin::Desktop,
        desktop_channel: Some(channel.clone()),
        navigation: None,
        events: Some(pages.clone()),
        process_done: done.clone(),
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
        let (page_done, navigation) = events::register_desktop(&pages, &session, &target);
        browser.done = page_done;
        browser.navigation = Some(navigation);
        if let Some(control) = &browser.control {
            events::register_control(&pages, control.clone());
        }
        request_in(&browser.tx, "Page.enable", json!({}), Some(session.clone())).await?;
        request(
            &browser.tx,
            "Target.setDiscoverTargets",
            json!({"discover":true}),
        )
        .await?;
        request(
            &browser.tx,
            "Target.setAutoAttach",
            json!({"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}),
        )
        .await?;
        request_in(
            &browser.tx,
            "Page.getFrameTree",
            json!({}),
            Some(session.clone()),
        )
        .await?;
        request_in(
            &browser.tx,
            "Network.enable",
            json!({}),
            Some(session.clone()),
        )
        .await?;
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
    use crate::server_browser::events::observe;
    use std::sync::{Arc, Mutex as SyncMutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
            let mut held: Option<Value> = None;
            loop {
                let mut line = String::new();
                let event = tokio::select! {event=incoming.recv()=>{let Some(event)=event else{break;};event},read=commands.read_line(&mut line)=>{
                    if !read.is_ok_and(|n|n>0){break;}
                    let command:Value=serde_json::from_str(&line).unwrap();captured.lock().unwrap().push(command.clone());if command["type"]!="cdp"{continue;}
                    let packet:Value=serde_json::from_str(command["message"].as_str().unwrap()).unwrap();
                    let envelope=|message:Value|json!({"type":"cdp","threadId":"thread","tabId":"tab","message":message.to_string()});
                    if packet["method"]=="Runtime.evaluate"&&packet["params"]["fixtureHold"]==true{
                        held=Some(packet.clone());let opening=envelope(json!({"method":"Page.javascriptDialogOpening","sessionId":packet["sessionId"],"params":{"type":"prompt","message":"Owned desktop prompt","defaultPrompt":"default"}}));
                        launcher.write_all(format!("{opening}\n").as_bytes()).await.unwrap();continue;
                    }
                    if packet["method"]=="Page.handleJavaScriptDialog"{
                        if let Some(pending)=held.take(){
                            let closed=envelope(json!({"method":"Page.javascriptDialogClosed","sessionId":pending["sessionId"],"params":{"result":packet["params"]["accept"]}}));
                            let reply=envelope(json!({"id":pending["id"],"sessionId":pending["sessionId"],"result":{"result":{"value":{"unblocked":true,"accepted":packet["params"]["accept"]}}}}));
                            launcher.write_all(format!("{closed}\n{reply}\n").as_bytes()).await.unwrap();
                        }
                    }
                    let unrelated=envelope(json!({"id":0,"result":{"unrelated":true}}));launcher.write_all(format!("{unrelated}\n").as_bytes()).await.unwrap();
                    let result=match string(&packet,"method"){
                        "Target.getTargets"=>json!({"targetInfos":if page{json!([{"type":"worker","targetId":"worker"},{"type":"page","targetId":"first-page"},{"type":"page","targetId":"second-page"}])}else{json!([])}}),
                        "Target.attachToTarget"=>json!({"sessionId":"opaque/session 😀"}),
                        "Page.getFrameTree"=>json!({"frameTree":{"frame":{"id":"first-page","url":"https://example.com/owned"}}}),
                        "Runtime.evaluate" if packet["params"]["fixtureBytes"].is_u64()=>json!({"result":{"value":packet["params"]["fixtureText"].as_str().unwrap_or("x").repeat(packet["params"]["fixtureBytes"].as_u64().unwrap()as usize)}}),
                        _=>json!({"accepted":true})};
                    let mut reply=json!({"id":packet["id"],"result":result});if let Some(session)=packet.get("sessionId"){reply["sessionId"]=session.clone();}envelope(reply)
                }};
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
            vec![
                "Target.getTargets",
                "Target.attachToTarget",
                "Page.enable",
                "Target.setDiscoverTargets",
                "Target.setAutoAttach",
                "Page.getFrameTree",
                "Network.enable"
            ]
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
            .open_desktop_owned(
                &peer.channel,
                "thread",
                "tab",
                "default",
                Some("provider:owned"),
            )
            .await
            .unwrap();
        let old = browser.page("thread", "tab").unwrap();
        old.control().take("viewer").wait().await.unwrap();
        assert_eq!(old.control().snapshot().generation, 1);
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
            .open_desktop_owned(
                &peer.channel,
                "thread",
                "tab",
                "default",
                Some("provider:owned"),
            )
            .await
            .unwrap();
        assert_eq!(
            browser.connection("thread", "tab").await,
            Some((Origin::Desktop, true))
        );
        let current = browser.page("thread", "tab").unwrap();
        assert_eq!(
            current.control().snapshot().agent_id.as_deref(),
            Some("provider:owned")
        );
        assert_eq!(current.control().snapshot().generation, 0);
        assert!(!browser.current(&old));
        assert!(old.command("Runtime.evaluate", json!({})).await.is_err());
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
    #[tokio::test]
    async fn desktop_dialog_answer_unblocks_evaluate_with_opaque_session_and_no_page_close() {
        let peer = setup(true).await;
        let browser = Arc::new(Browsers::default());
        browser
            .open_desktop(&peer.channel, "thread", "tab", "default")
            .await
            .unwrap();
        let owned = browser.clone();
        let eval = tokio::spawn(async move {
            owned
                .command(
                    "thread",
                    "tab",
                    "Runtime.evaluate",
                    json!({"fixtureHold":true}),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !packets(&peer)
                .iter()
                .any(|packet| packet["params"]["fixtureHold"] == true)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while browser.dialog_status("thread", "tab").unwrap()["message"]
                != "Owned desktop prompt"
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(!eval.is_finished());
        tokio::time::timeout(
            Duration::from_secs(2),
            browser.answer_dialog("thread", "tab", false, None),
        )
        .await
        .unwrap()
        .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), eval)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            result["result"]["value"],
            json!({"unblocked":true,"accepted":false})
        );
        assert_eq!(browser.dialog_status("thread", "tab").unwrap(), Value::Null);
        peer.events.send(json!({"type":"cdp","threadId":"thread","tabId":"tab","message":r#"{"method":"Page.javascriptDialogOpening","sessionId":"opaque/session 😀","params":{"type":"prompt","message":"\ud800A\udfff🙂","defaultPrompt":"\udc00"}}"#})).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let state = browser.dialog_state("thread", "tab").unwrap();
                if state.json_text().stringify().contains(r#"\ud800A\udfff"#) {
                    assert!(state.json_text().stringify().contains(r#"\udc00"#));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        browser
            .answer_dialog("thread", "tab", false, None)
            .await
            .unwrap();
        assert_eq!(
            browser.connection("thread", "tab").await,
            Some((Origin::Desktop, true))
        );
        browser.shutdown().await;
        assert!(packets(&peer)
            .iter()
            .all(|packet| packet["method"] != "Target.closeTarget"
                && packet["method"] != "Browser.close"));
        assert!(peer
            .messages
            .lock()
            .unwrap()
            .iter()
            .any(|message| message["type"] == "release"));
        peer.task.abort();
    }
    #[tokio::test]
    async fn desktop_native_sized_reply_crosses_private_fd_endpoint_and_owned_actor() {
        let peer = setup(true).await;
        let browser = Browsers::default();
        browser
            .open_desktop(&peer.channel, "thread", "tab", "default")
            .await
            .unwrap();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/preview-transport-limits-4288.json"
        ))
        .unwrap();
        assert_eq!(fixture["playwright"]["receiveBytes"], MAX_REPLY_BYTES);
        assert_eq!(fixture["nativeSerializedValueBudget"], 64 * 1024 * 1024);
        for index in [2, 3, 4] {
            let case = &fixture["cases"][index];
            assert_eq!(case["expected"]["fits"], true);
            let symbol = case["input"]["text"].as_str().unwrap();
            let count = case["input"]["repeat"].as_u64().unwrap() as usize;
            let result = browser
                .command(
                    "thread",
                    "tab",
                    "Runtime.evaluate",
                    json!({"fixtureBytes":count,"fixtureText":symbol}),
                )
                .await
                .unwrap();
            let text = result["result"]["value"].as_str().unwrap();
            assert_eq!(text.len(), symbol.len() * count);
            let expected = symbol.chars().next().unwrap();
            assert!(text.chars().all(|character| character == expected));
        }
        assert_eq!(
            browser.connection("thread", "tab").await,
            Some((Origin::Desktop, true))
        );
        browser.shutdown().await;
        peer.task.abort();
    }
    #[test]
    fn actual_native_relay_frames_require_auto_attach_and_preserve_opaque_sessions() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/preview-navigation-4288.json"
        ))
        .unwrap();
        assert_eq!(fixture["relayCases"].as_array().unwrap().len(), 2);
        for case in fixture["relayCases"].as_array().unwrap() {
            let session = case["sessionId"].as_str().unwrap();
            let (tx, rx) = tokio::sync::watch::channel(Navigation::default());
            let mut frame = Some("main-frame".to_string());
            for event in case["events"].as_array().unwrap() {
                assert!(!observe(
                    event,
                    Some(session),
                    Some("page"),
                    &mut frame,
                    &tx
                ));
            }
            assert_eq!(
                rx.borrow().revision,
                if case["autoAttach"] == true { 1 } else { 0 }
            );
            assert_eq!(case["closes"], 0);
            assert!(case["calls"]
                .as_array()
                .unwrap()
                .iter()
                .any(|call| call["method"] == "Page.getFrameTree" && call["owner"] == session));
            assert!(!observe(
                &json!({"method":"Inspector.detached","sessionId":"unrelated-child"}),
                Some(session),
                Some("page"),
                &mut frame,
                &tx
            ));
        }
    }
}
