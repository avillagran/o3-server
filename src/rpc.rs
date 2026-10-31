//! Effect RPC: multiplexed, one unacknowledged chunk per stream, SQLite replay cursors.
use crate::{events, scopes, settings, store::State};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
pub const SERVER_VERSION: &str = concat!("o3-", env!("CARGO_PKG_VERSION"));
struct Subscription {
    id: Value,
    tag: String,
    tid: Option<String>,
    cursor: i64,
    waiting: bool,
    initial: Option<Value>,
    snapshot: Option<crate::snapshot::Receiver>,
    marker: bool,
    version: u64,
}
fn exit(id: &Value, result: Result<Value, Value>) -> Value {
    json!({"_tag":"Exit","requestId":id,"exit":match result {Ok(v)=>json!({"_tag":"Success","value":v}),Err(e)=>json!({"_tag":"Failure","cause":[{"_tag":"Fail","error":e}]})}})
}
fn error(message: String) -> Value {
    json!({"_tag":"RpcNotImplemented","message":message})
}
pub fn authorize(session: &Value, tag: &str, p: &Value) -> Result<(), Value> {
    let tag = if tag == "server.subscribeServerConfig" {
        "subscribeServerConfig"
    } else {
        tag
    };
    let check = |scope: &str| {
        if session["scopes"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == scope))
        {
            Ok(())
        } else {
            Err(
                json!({"_tag":"EnvironmentAuthorizationError","message":format!("The authenticated token is missing required scope: {scope}."),"requiredScope":scope}),
            )
        }
    };
    if tag == "server.updateSettings" {
        let patch = p["patch"].as_object();
        let providers = patch.is_some_and(|m| {
            m.keys().any(|k| {
                matches!(
                    k.as_str(),
                    "providers" | "providerInstances" | "usageLimitSources"
                )
            })
        }) || p.get("providerInstanceMutation").is_some();
        let settings = patch.is_some_and(|m| {
            m.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "providers" | "providerInstances" | "usageLimitSources"
                )
            })
        }) || !providers;
        if settings {
            check("settings:write")?;
        }
        if providers {
            check("providers:manage")?;
        }
    } else if tag == "projects.mutate" {
        check("orchestration:operate")?;
        if p.get("scripts").is_some() {
            check("settings:write")?;
        }
    } else if let Some(scope) = scopes::required(tag) {
        check(scope)?;
    }
    Ok(())
}
fn dispatch(s: &Arc<State>, tag: &str, p: &Value) -> Result<Value, Value> {
    match tag {
        "server.getConfig"=>settings::config(s),
        "server.getSettings"=>return settings::get(s).map_err(|message|json!({"_tag":"ServerSettingsError","settingsPath":s.dir.join("settings.json"),"operation":"read","message":message})),
        "server.updateSettings"=>return settings::update(s,p).map_err(|message|json!({"_tag":"ServerSettingsError","settingsPath":s.dir.join("settings.json"),"operation":"write","message":message})),
        "projects.mutate"=>return events::mutate_project(s,p).map_err(|message|json!({"_tag":"ProjectMutationError","message":message})),
        "orchestration.launchThread"=>return events::launch(s,p).map_err(|message|json!({"_tag":"OrchestrationV2ThreadLaunchError","commandId":p["commandId"],"projectId":p["projectId"],"message":message})),
        "orchestration.dispatchCommand"=>return events::dispatch(s,p).map_err(|m|json!({"_tag":"OrchestrationV2DispatchCommandError","commandId":p["commandId"],"message":m})),
        _=>Err(format!("o3-server does not implement {tag}")),
    }.map_err(error)
}
async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    v: Value,
) -> Result<(), ()> {
    ws.send(Message::Text(v.to_string().into()))
        .await
        .map_err(|_| ())
}
pub async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    mut ws: WebSocketStream<S>,
    s: Arc<State>,
    session: Value,
) {
    let mut changed = s.changed.subscribe();
    let mut subscriptions: BTreeMap<String, Subscription> = BTreeMap::new();
    let mut eof = false;
    let mut expiry = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        if !s.session_live(&session) {
            let _ = ws.close(None).await;
            break;
        }
        for sub in subscriptions.values_mut().filter(|sub| !sub.waiting) {
            if let Some(mut rx) = sub.snapshot.take() {
                use tokio_tungstenite::tungstenite::protocol::frame::{
                    coding::{Data, OpCode},
                    Frame,
                };
                let mut first = true;
                while let Some(part) = rx.recv().await {
                    let Ok(bytes) = part else { return };
                    let opcode = OpCode::Data(if first { Data::Text } else { Data::Continue });
                    first = false;
                    if ws
                        .send(Message::Frame(Frame::message(bytes, opcode, false)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                if ws
                    .send(Message::Frame(Frame::message(
                        Vec::new(),
                        OpCode::Data(Data::Continue),
                        true,
                    )))
                    .await
                    .is_err()
                {
                    return;
                }
                sub.waiting = true;
                continue;
            }
            let item = if let Some(v) = sub.initial.take() {
                Some(v)
            } else if matches!(
                sub.tag.as_str(),
                "server.subscribeServerConfig" | "subscribeServerConfig"
            ) {
                let version = *changed.borrow();
                if version != sub.version {
                    sub.version = version;
                    settings::config(&s)
                        .ok()
                        .map(|v| json!({"version":1,"type":"snapshot","config":v}))
                } else {
                    None
                }
            } else {
                match events::next(
                    &s,
                    sub.cursor,
                    sub.tid.as_deref(),
                    sub.tag == "orchestration.subscribeShell",
                ) {
                    Ok(Some((seq, v))) => {
                        sub.cursor = seq;
                        Some(if sub.tid.is_some() {
                            json!({"kind":"event","sequence":seq,"event":v})
                        } else {
                            v
                        })
                    }
                    Ok(None) if sub.marker => {
                        sub.marker = false;
                        Some(json!({"kind":"synchronized"}))
                    }
                    _ => None,
                }
            };
            if let Some(v) = item {
                if send(
                    &mut ws,
                    json!({"_tag":"Chunk","requestId":sub.id,"values":[v]}),
                )
                .await
                .is_err()
                {
                    return;
                }
                sub.waiting = true;
            }
        }
        if eof && subscriptions.is_empty() {
            let _ = ws.close(None).await;
            break;
        }
        tokio::select! {
            _=changed.changed()=>{},_=expiry.tick()=>{},
            message=ws.next()=>{
                let Some(Ok(message))=message else {break};
                match message {
                    Message::Ping(b)=>{if ws.send(Message::Pong(b)).await.is_err(){break}},Message::Close(_)=>break,
                    Message::Text(text)=>{
                        let Ok(frame)=serde_json::from_str::<Value>(&text) else {break};
                        let frames=match frame {Value::Array(a)=>a,v=>vec![v]};if frames.len()>128{break;}
                        for frame in frames {
                            let id=frame.get("id").cloned().unwrap_or(Value::Null);let key=frame["requestId"].to_string();
                            match frame["_tag"].as_str().unwrap_or("") {
                                "Ping"=>if send(&mut ws,json!({"_tag":"Pong"})).await.is_err(){return},
                                "Ack"=>{if let Some(sub)=subscriptions.get_mut(&key){sub.waiting=false;}},
                                "Interrupt"=>{if let Some(sub)=subscriptions.remove(&key){let v=json!({"_tag":"Exit","requestId":sub.id,"exit":{"_tag":"Failure","cause":[{"_tag":"Interrupt","fiberId":frame["interruptors"]}]}});if send(&mut ws,v).await.is_err(){return}}},
                                "Eof"=>eof=true,
                                "Request" if !eof=>{
                                    let tag=frame["tag"].as_str().unwrap_or("");let p=&frame["payload"];
                                    if let Err(e)=authorize(&session,tag,p){if send(&mut ws,exit(&id,Err(e))).await.is_err(){return}continue;}
                                    if matches!(tag,"orchestration.subscribeShell"|"orchestration.subscribeThread"|"server.subscribeServerConfig"|"subscribeServerConfig") {
                                        if subscriptions.len()>=64||subscriptions.contains_key(&id.to_string()) {if send(&mut ws,exit(&id,Err(error("subscription limit or duplicate id".into())))).await.is_err(){return}continue;}
                                        // Register notification receiver before any snapshot; no await until cursor is installed.
                                        let version=*changed.borrow();let seq=events::sequence(&s.db.lock().unwrap()).unwrap_or(0);let after=p["afterSequence"].as_i64().filter(|v|*v<=seq);
                                        let tid=if tag=="orchestration.subscribeThread" {p["threadId"].as_str().map(str::to_owned)}else{None};
                                        let config=matches!(tag,"server.subscribeServerConfig"|"subscribeServerConfig");
                                        if tag=="orchestration.subscribeThread" && tid.is_none() {if send(&mut ws,exit(&id,Err(error("threadId required".into())))).await.is_err(){return}continue;}
                                        let mut snapshot=None;let mut boundary=seq;
                                        if !config && after.is_none() {
                                            let opts=crate::snapshot::Options{bounded:p["acceptBoundedSnapshot"]==true,dedupe:p["acceptDedupedBodies"]==true,compact_items:p["acceptCompactTurnItems"]==true,compact_ids:p["acceptCompactIds"]==true};
                                            match crate::snapshot::start(&s,tid.clone(),opts,Some(id.clone())) {
                                                Ok((seq,rx))=>{boundary=seq;snapshot=Some(rx);},
                                                Err(e)=>{if send(&mut ws,exit(&id,Err(error(e)))).await.is_err(){return}continue;}
                                            }
                                        }
                                        let initial=if config {settings::config(&s).ok().map(|c|json!({"version":1,"type":"snapshot","config":c}))}else{None};
                                        subscriptions.insert(id.to_string(),Subscription{id,tag:tag.into(),tid,cursor:after.unwrap_or(boundary),waiting:false,initial,snapshot,marker:p["requestCompletionMarker"]==true,version});
                                    }else if send(&mut ws,exit(&id,dispatch(&s,tag,p))).await.is_err(){return}
                                },_=>{}
                            }
                        }
                    },_=>{}
                }
            }
        }
    }
}
