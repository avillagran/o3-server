//! Genuine server transactions and original pinned V8 open-history computation.
mod support;
use serde_json::{json,Value};
fn create(c:&t3_protocol::Connection){
    c.rpc("orchestration.dispatchCommand",json!({"type":"thread.create","commandId":"create-open","threadId":"open","projectId":"p1","title":"Open","modelSelection":{"instanceId":"scripted","model":"test","options":[]},"runtimeMode":"full-access","interactionMode":"default","branch":null,"worktreePath":null})).unwrap();
}
fn thread(s:&support::Server)->Value{
    let db=s.state.db.lock().unwrap();
    let raw:String=db.query_row("SELECT body FROM records WHERE kind='thread' AND id='open'",[],|r|r.get(0)).unwrap();serde_json::from_str(&raw).unwrap()
}
#[test]
fn visits_are_forward_only_and_do_not_change_activity(){
    let s=support::start(|_|{});let c=s.connect();create(&c);let before=thread(&s);
    for (id,at) in [("forward","2099-01-01T01:00:00+01:00"),("backward","2020-01-01T00:00:00Z")]{
        c.rpc("orchestration.dispatchCommand",json!({"type":"thread.visit","commandId":id,"threadId":"open","visitedAt":at})).unwrap();
    }
    let after=thread(&s);assert_eq!(after["lastVisitedAt"],"2099-01-01T00:00:00.000Z");assert_eq!(after["updatedAt"],before["updatedAt"]);assert!(after["opens"].is_null());
}
#[test]
fn actual_original_opens_use_server_event_time_dedupe_and_persist_picks(){
    let node=std::env::var("O3_ADMITTED_NODE").expect("Private admitted Node mount required");
    let s=support::start(|state|crate_install(state,&node));let c=s.connect();create(&c);let before=thread(&s);
    let visit=|id:&str,query:&str|json!({"type":"thread.visit","commandId":id,"threadId":"open","visitedAt":"2999-01-01T00:00:00Z","opened":true,"searchQuery":query});
    let command=visit("pick-one","  CÁFÉ  日本 😀 ");let first=c.rpc("orchestration.dispatchCommand",command.clone()).unwrap();let opened=thread(&s);assert_eq!(opened["opens"]["score"],1);assert_eq!(opened["opens"]["picks"][0]["query"],"cafe 日本 😀");assert_eq!(opened["updatedAt"],before["updatedAt"]);
    let db=s.state.db.lock().unwrap();let body:String=db.query_row("SELECT body FROM events WHERE sequence=?",[first["sequence"].as_i64().unwrap()],|r|r.get(0)).unwrap();drop(db);let event:Value=serde_json::from_str(&body).unwrap();let occurred=chrono::DateTime::parse_from_rfc3339(event["occurredAt"].as_str().unwrap()).unwrap().timestamp_millis();assert_eq!(opened["opens"]["openedAt"],occurred);assert!(occurred<32503680000000i64);
    assert_eq!(c.rpc("orchestration.dispatchCommand",command).unwrap(),first);assert_eq!(thread(&s),opened);
    c.rpc("orchestration.dispatchCommand",visit("pick-two","next")).unwrap();let next=thread(&s);assert_eq!(next["opens"]["openedAt"],opened["opens"]["openedAt"]);assert_eq!(next["opens"]["score"],1);assert_eq!(next["opens"]["picks"].as_array().unwrap().len(),2);assert_eq!(next["opens"]["picks"][0]["query"],"next");
    let shell=o3_server::events::shell(&s.state).unwrap();assert!(shell.to_string().contains("cafe 日本 😀"));
}
fn crate_install(state:&o3_server::store::State,node:&str){o3_server::thread_open::install(state,&["--native-search-node".into(),node.into()]).unwrap();}
