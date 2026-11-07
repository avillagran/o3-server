//! One private worker connection. The nonce endpoint and resource receipts are
//! constructed by the captured owner; no MCP argument selects an upstream URL.
use super::*;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio_tungstenite::tungstenite::{self, Message};
const MAX_BYTES: usize = 4 * 1024 * 1024;

pub struct NativeGateway {
    page: NativePage,
    endpoint: String,
    runtime_receipt: String,
    cleanup_receipt: String,
    stop: tokio::sync::watch::Sender<bool>,
    task: Mutex<Option<tokio::task::JoinHandle<NativeCloseReceipt>>>,
}
fn secret() -> Result<String, Value> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|_| fault("entropy"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
impl NativePage {
    pub async fn gateway(&self) -> Result<NativeGateway, Value> {
        use std::sync::atomic::Ordering;
        if self
            .inner
            .gateway_bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(fault("already_bound"));
        }
        struct Initializing(Arc<Owner>, bool);
        impl Drop for Initializing {
            fn drop(&mut self) {
                if !self.1 {
                    self.0.gateway_bound.store(false, Ordering::Release);
                }
            }
        }
        let mut initializing = Initializing(self.inner.clone(), false);
        if *self.inner.revoked.borrow() {
            return Err(fault("closed"));
        }
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| fault("listener"))?;
        let address = listener.local_addr().map_err(|_| fault("listener"))?;
        let path = format!("/{}{}", secret()?, secret()?);
        let runtime_receipt = secret()?;
        let cleanup_receipt = secret()?;
        let (stop, stopping) = tokio::sync::watch::channel(false);
        let page = self.clone();
        let runtime = runtime_receipt.clone();
        let cleanup = cleanup_receipt.clone();
        let endpoint = format!("ws://{address}{path}");
        let host = address.to_string();
        let task = tokio::spawn(async move {
            serve(page, listener, path, host, runtime, cleanup, stopping).await
        });
        initializing.1 = true;
        Ok(NativeGateway {
            page: self.clone(),
            endpoint,
            runtime_receipt,
            cleanup_receipt,
            stop,
            task: Mutex::new(Some(task)),
        })
    }
    pub(super) fn fence(&self) {
        self.inner.grants.lock().unwrap().close();
        self.inner.revoked.send_replace(true);
        self.inner.wake.notify_one();
        self.inner.changed.notify_waiters();
    }
}
impl NativeGateway {
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    /// These owner-only capabilities belong in the sealed initialization frame,
    /// never in model input or a public MCP result.
    pub fn runtime_receipt(&self) -> &str {
        &self.runtime_receipt
    }
    pub fn cleanup_receipt(&self) -> &str {
        &self.cleanup_receipt
    }
    /// Seal the owner resources into trusted initialization, outside operation
    /// input. A caller-supplied replacement capability is refused.
    pub fn configure_worker(
        &self,
        spec: &mut crate::native_page_worker::WorkerSpec,
    ) -> Result<(), crate::native_page_worker::Fault> {
        let configuration = spec
            .configuration
            .as_object_mut()
            .ok_or(crate::native_page_worker::Fault::InvalidInstallation)?;
        if configuration.contains_key("ownerScopes") {
            return Err(crate::native_page_worker::Fault::InvalidInstallation);
        }
        configuration.insert(
            "ownerScopes".into(),
            json!({
                "runtimeReceipt":self.runtime_receipt,
                "cleanupReceipt":self.cleanup_receipt,
            }),
        );
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<NativeCloseReceipt, crate::native_page_worker::Fault> {
        self.page.fence();
        self.stop.send_replace(true);
        let mut task = self.task.lock().await;
        let Some(mut running) = task.take() else {
            return Ok(self.page.inner.receipt());
        };
        match tokio::time::timeout(Duration::from_secs(20), &mut running).await {
            Ok(Ok(receipt)) => Ok(receipt),
            _ => {
                running.abort();
                let _ = running.await;
                self.page.inner.grants.lock().unwrap().mark_unknown();
                Err(crate::native_page_worker::Fault::CleanupUnknown)
            }
        }
    }
}
impl Drop for NativeGateway {
    fn drop(&mut self) {
        self.page.fence();
        self.stop.send_replace(true);
    }
}
impl crate::native_page_worker::GrantOwner for NativeGateway {
    fn target_id(&self) -> &str {
        self.page.target()
    }
    fn endpoint(&self) -> &str {
        &self.endpoint
    }
    fn revocation(&self) -> tokio::sync::watch::Receiver<bool> {
        self.page.inner.revoked.subscribe()
    }
    fn revoke(&self) {
        self.page.fence();
    }
    fn close(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), crate::native_page_worker::Fault>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            if self.shutdown().await?.clean {
                Ok(())
            } else {
                Err(crate::native_page_worker::Fault::CleanupUnknown)
            }
        })
    }
}
struct Incoming {
    prepared: Option<Ticket>,
    id: u64,
    method: String,
    params: JavaScriptJson,
    session: Option<String>,
    receipt: String,
    operation: Option<String>,
}
struct Fields(BTreeMap<String, Box<RawValue>>);
impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> serde::de::Visitor<'de> for Object {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an unambiguous owner command")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Fields, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((name, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    if fields.len() >= 6 || fields.insert(name, value).is_some() {
                        return Err(serde::de::Error::custom("ambiguous owner command"));
                    }
                }
                Ok(Fields(fields))
            }
        }
        decoder.deserialize_map(Object)
    }
}
fn scalar<T: serde::de::DeserializeOwned>(fields: &mut Fields, key: &str) -> Result<T, ()> {
    serde_json::from_str(fields.0.remove(key).ok_or(())?.get()).map_err(|_| ())
}
fn decode(bytes: &[u8]) -> Result<Incoming, ()> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(());
    }
    let mut fields: Fields = serde_json::from_slice(bytes).map_err(|_| ())?;
    let id = scalar::<u64>(&mut fields, "id")?;
    let method = scalar::<String>(&mut fields, "method")?;
    let receipt = scalar::<String>(&mut fields, "__o3OwnerReceipt")?;
    if id == 0
        || id > 9_007_199_254_740_991
        || method.is_empty()
        || method.len() > 128
        || receipt.len() != 48
        || !receipt
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(());
    }
    let session = if fields.0.contains_key("sessionId") {
        Some(scalar::<String>(&mut fields, "sessionId")?)
    } else {
        None
    };
    let operation = if fields.0.contains_key("__o3OwnerOperation") {
        Some(scalar::<String>(&mut fields, "__o3OwnerOperation")?)
    } else {
        None
    };
    let params = JavaScriptJson::parse(
        fields
            .0
            .remove("params")
            .map_or_else(|| "{}".into(), |raw| raw.get().to_owned()),
    )
    .map_err(|_| ())?;
    if !fields.0.is_empty() {
        return Err(());
    }
    Ok(Incoming {
        prepared: None,
        id,
        method,
        params,
        session,
        receipt,
        operation,
    })
}
#[derive(Serialize)]
struct Reply<'a> {
    id: u64,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a JavaScriptJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a JavaScriptJson>,
}
fn reply(id: u64, session: Option<&str>, result: Result<Packet, Value>) -> Message {
    use crate::native_page_worker::cdp::Payload;
    let unavailable = JavaScriptJson::parse(
        r#"{"code":-32000,"message":"Command unavailable for this owned Page"}"#.into(),
    )
    .unwrap();
    let (value, error) = match &result {
        Ok(packet) => match packet.payload() {
            Some(Payload::Result(value)) => (Some(value), None),
            Some(Payload::Error(error)) => (None, Some(error)),
            _ => (None, Some(&unavailable)),
        },
        Err(_) => (None, Some(&unavailable)),
    };
    Message::Text(
        serde_json::to_string(&Reply {
            id,
            session,
            result: value,
            error,
        })
        .unwrap()
        .into(),
    )
}
async fn command(
    page: NativePage,
    mut request: Incoming,
    runtime: String,
    cleanup: String,
    previous: Option<oneshot::Receiver<()>>,
    dispatch: Dispatch,
) -> Message {
    if let Some(previous) = previous {
        let _ = previous.await;
    }
    let mut flow = Flow {
        cleanup: None,
        enqueued: Some(dispatch),
    };
    let id = request.id;
    let session = request.session.clone();
    let result = if request.receipt == cleanup {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Detach {
            #[serde(rename = "sessionId")]
            session: String,
        }
        match serde_json::from_str::<Detach>(request.params.json()) {
            Ok(detach)
                if request.method == "Target.detachFromTarget" && request.operation.is_none() =>
            {
                let reservation = page
                    .inner
                    .grants
                    .lock()
                    .unwrap()
                    .cleanup_detach(&detach.session);
                match reservation {
                    Ok(reservation) => {
                        let wire_session = reservation.session.clone();
                        page.inner
                            .send(
                                None,
                                "Target.detachFromTarget",
                                request.params,
                                wire_session,
                                Duration::from_secs(15),
                                {
                                    flow.cleanup = Some(reservation);
                                    flow
                                },
                            )
                            .await
                    }
                    Err(_) => Err(fault("cleanup_scope")),
                }
            }
            _ => Err(fault("cleanup_scope")),
        }
    } else if request.receipt == runtime && request.operation.is_none() {
        page.inner
            .send(
                None,
                &request.method,
                request.params,
                request.session,
                Duration::from_secs(121),
                flow,
            )
            .await
    } else {
        match request.prepared.take() {
            Some(ticket) => {
                page.inner
                    .send(
                        Some(ticket),
                        &request.method,
                        request.params,
                        request.session,
                        Duration::from_secs(121),
                        flow,
                    )
                    .await
            }
            None => Err(fault("receipt_operation")),
        }
    };
    reply(id, session.as_deref(), result)
}
struct Admission {
    path: String,
    host: String,
}
impl tungstenite::handshake::server::Callback for Admission {
    fn on_request(
        self,
        request: &tungstenite::handshake::server::Request,
        response: tungstenite::handshake::server::Response,
    ) -> Result<
        tungstenite::handshake::server::Response,
        tungstenite::handshake::server::ErrorResponse,
    > {
        if request.uri().path() != self.path
            || request.uri().query().is_some()
            || request.headers().contains_key("origin")
            || request
                .headers()
                .get("host")
                .and_then(|host| host.to_str().ok())
                != Some(self.host.as_str())
        {
            return Err(tungstenite::http::Response::builder()
                .status(403)
                .body(Some("Owned Page connection refused".into()))
                .unwrap());
        }
        Ok(response)
    }
}
async fn serve(
    page: NativePage,
    listener: tokio::net::TcpListener,
    path: String,
    host: String,
    runtime: String,
    cleanup: String,
    mut stopping: tokio::sync::watch::Receiver<bool>,
) -> NativeCloseReceipt {
    let mut target_done = page.inner.done.clone();
    let mut events = page.events();
    let socket = loop {
        let accepted = tokio::select! { biased; _=stopping.changed()=>break None,_=target_done.changed()=>break None,accepted=listener.accept()=>accepted };
        let Ok((stream, peer)) = accepted else {
            break None;
        };
        if !peer.ip().is_loopback() {
            continue;
        }
        let expected_path = path.clone();
        let expected_host = host.clone();
        let handshake = tokio_tungstenite::accept_hdr_async_with_config(
            stream,
            Admission {
                path: expected_path,
                host: expected_host,
            },
            Some(
                tungstenite::protocol::WebSocketConfig::default()
                    .max_message_size(Some(MAX_BYTES))
                    .max_frame_size(Some(MAX_BYTES)),
            ),
        );
        match tokio::time::timeout(Duration::from_secs(5), handshake).await {
            Ok(Ok(socket)) => break Some(socket),
            _ => continue,
        }
    };
    drop(listener);
    if let Some(socket) = socket {
        let (mut output, mut input) = socket.split();
        let mut calls = tokio::task::JoinSet::new();
        let mut last_id = 0;
        let mut predecessor = None;
        loop {
            tokio::select! {
                _=stopping.changed()=>break,
                _=target_done.changed()=>break,
                completed=calls.join_next(),if !calls.is_empty()=>{
                    match completed { Some(Ok(message))=>if output.send(message).await.is_err(){break;},_=>break }
                },
                event=events.recv()=>{
                    match event { Ok(packet)=>{if output.send(Message::Text(serde_json::to_string(&packet).unwrap().into())).await.is_err(){break;}},Err(_)=>{page.fence();break;} }
                },
                incoming=input.next()=>{
                    let Some(Ok(message))=incoming else { break; };
                    match message {
                        Message::Text(text)=>{
                            let Ok(mut request)=decode(text.as_bytes()) else { page.fence();break; };
                            if request.id<=last_id { page.fence();break; }
                            last_id=request.id;
                            let operation=request.operation.as_deref().and_then(NativeOperation::from_tool);
                            if request.method=="O3Owner.endScope" {
                                let sealed = if request.session.is_none() && request.params.json()=="{}" {
                                    operation.ok_or_else(||fault("receipt_operation"))
                                        .and_then(|operation|page.seal_receipt(&request.receipt,operation))
                                } else { Err(fault("scope_end")) };
                                let page=page.clone();
                                calls.spawn(async move {
                                    let result=match sealed {
                                        Ok(mut drained)=>{
                                            let waiting=async{while !*drained.borrow_and_update(){if drained.changed().await.is_err(){return Err(fault("scope_end"));}} Ok(())};
                                            match tokio::time::timeout(Duration::from_secs(121),waiting).await {
                                                Ok(result)=>result,
                                                Err(_)=>{page.inner.grants.lock().unwrap().mark_unknown();page.fence();Err(fault("scope_end_unknown"))}
                                            }
                                        },
                                        Err(error)=>Err(error),
                                    };
                                    match result {
                                        Ok(())=>Message::Text(json!({"id":request.id,"result":{"scopeEnded":true}}).to_string().into()),
                                        Err(error)=>reply(request.id,None,Err(error)),
                                    }
                                });
                            } else {
                                if request.receipt!=runtime && request.receipt!=cleanup {
                                    request.prepared=operation.and_then(|operation|page.prepare_receipt(&request.receipt,operation).ok());
                                }
                                // CDP has no invented eight-call rejection limit. The
                                // actor's ordered bounded queue supplies backpressure;
                                // all unresolved tasks remain owned through shutdown.
                                let previous = predecessor.take();
                                let (advance, next) = oneshot::channel();
                                predecessor = Some(next);
                                calls.spawn(command(page.clone(),request,runtime.clone(),cleanup.clone(),previous,Dispatch(Some(advance))));
                            }
                        },
                        Message::Ping(value)=>{if output.send(Message::Pong(value)).await.is_err(){break;}},
                        Message::Pong(_)=>{},
                        _=>break,
                    }
                },
            }
        }
        calls.abort_all();
        while calls.join_next().await.is_some() {}
        let _ = output.close().await;
    }
    page.fence();
    page.close(Duration::from_secs(15)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tungstenite::handshake::server::Callback;
    #[test]
    fn gateway_metadata_is_strict_while_application_json_keeps_utf16_escapes() {
        let receipt = "a".repeat(48);
        let raw = format!(
            r#"{{"id":1,"method":"Runtime.evaluate","params":{{"expression":"'\ud800'"}},"sessionId":"owned","__o3OwnerReceipt":"{receipt}","__o3OwnerOperation":"preview_evaluate"}}"#
        );
        let request = decode(raw.as_bytes()).unwrap();
        assert!(request.params.json().contains("\\ud800"));
        assert!(serde_json::from_str::<Value>(request.params.json()).is_err());
        for malformed in [
            raw.replace("\"id\":1", "\"id\":1,\"id\":2"),
            raw.replace("\"id\":1", "\"id\":9007199254740992"),
            raw.replace("\"owned\"", "\"\\ud800\""),
            raw.replace("Runtime.evaluate", ""),
            raw.replace("preview_evaluate", "\\ud800"),
            raw.replace("\"id\":1", "\"unknown\":true,\"id\":1"),
        ] {
            assert!(decode(malformed.as_bytes()).is_err());
        }
    }
    #[test]
    fn gateway_reply_remaps_only_metadata_and_preserves_raw_browser_payload() {
        let packet = Packet::decode(
            br#"{"id":9,"sessionId":"owned","result":{"result":{"value":{"\ud800":"\udfff"}}}}"#,
            MAX_BYTES,
        )
        .unwrap();
        let Message::Text(reply) = reply(2, Some("owned"), Ok(packet.clone())) else {
            panic!("text reply")
        };
        let received = Packet::decode(reply.as_bytes(), MAX_BYTES).unwrap();
        assert_eq!(received.id(), Some(2));
        assert_eq!(received.session(), Some("owned"));
        assert_eq!(received.payload(), packet.payload());
    }
    #[test]
    fn gateway_upgrade_rejects_browser_origins_wrong_hosts_paths_and_queries() {
        for (uri, host, origin, valid) in [
            ("/private-owned-path", "127.0.0.1:1234", None, true),
            ("/private-owned-path", "127.0.0.1:1234", Some("null"), false),
            ("/private-owned-path", "localhost:1234", None, false),
            ("/private-owned-path?x=1", "127.0.0.1:1234", None, false),
            ("/other", "127.0.0.1:1234", None, false),
        ] {
            let mut request = tungstenite::http::Request::builder()
                .uri(uri)
                .header("host", host);
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            let response = Admission {
                path: "/private-owned-path".into(),
                host: "127.0.0.1:1234".into(),
            }
            .on_request(
                &request.body(()).unwrap(),
                tungstenite::http::Response::new(()),
            );
            assert_eq!(response.is_ok(), valid);
        }
    }
}
