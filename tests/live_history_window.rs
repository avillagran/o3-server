mod support;
use serde_json::{json, Value};
use std::collections::HashSet;

#[test]
fn updated_streaming_answer_remains_visible_after_hundreds_of_tools_and_history_pages_stay_complete(
) {
    for reposition in [false, true] {
        let server = support::start(|state| {
            let db = state.db.lock().unwrap();
            let put = |kind: &str, id: &str, body: Value| {
                db.execute(
                    "INSERT INTO records(kind,id,thread_id,body) VALUES(?1,?2,'t',?3)",
                    rusqlite::params![kind, id, body.to_string()],
                )
                .unwrap();
            };
            put(
                "thread",
                "t",
                json!({"id":"t","providerInstanceId":"scripted","createdAt":"2026-11-06T12:00:00.000Z","updatedAt":"2026-11-06T18:00:00.000Z","deletedAt":null}),
            );
            put("runs", "r", json!({"id":"r","status":"completed"}));
            put(
                "messages",
                "answer",
                json!({"id":"answer","runId":"r","role":"assistant","text":"The current answer is still visible.","streaming":true,"attachments":[],"createdAt":"2026-11-06T12:00:00.000Z","updatedAt":"2026-11-06T18:00:00.000Z"}),
            );
            let answer = json!({"id":"item-answer","runId":"r","type":"assistant_message","messageId":"answer","text":"The current answer is still visible.","streaming":true,"ordinal":0});
            put("turnItems", "item-answer", answer.clone());
            put(
                "visibleTurnItems",
                "item-answer",
                json!({"position":0,"visibility":"local","sourceThreadId":"t","sourceItemId":"item-answer","item":answer}),
            );
            for n in 1..=600 {
                let id = format!("tool-{n}");
                let item = json!({"id":id,"runId":"r","type":"command_execution","ordinal":n,"status":"completed","command":"private fixture","output":"","exitCode":0});
                put("turnItems", &id, item.clone());
                put(
                    "visibleTurnItems",
                    &id,
                    json!({"position":n,"visibility":"local","sourceThreadId":"t","sourceItemId":id,"item":item}),
                );
            }
            // An update changes the canonical position while the SQLite row keeps
            // its original age, exactly as the long-running live thread does.
            if reposition {
                db.execute("UPDATE records SET body=json_set(body,'$.position',1000,'$.item.ordinal',1000) WHERE kind='visibleTurnItems' AND id='item-answer'",[]).unwrap();
                db.execute("UPDATE records SET body=json_set(body,'$.ordinal',1000) WHERE kind='turnItems' AND id='item-answer'",[]).unwrap();
            }
        });
        let client = server.connect();
        // Begin the synthetic live run after startup recovery has finished.
        server.state.db.lock().unwrap().execute("UPDATE records SET body=json_set(body,'$.status','running') WHERE kind='runs' AND id='r'",[]).unwrap();
        let plain = client
            .request_json("GET", "/api/orchestration/threads/t/bounded", None, true)
            .unwrap();
        assert_eq!(
            plain["projection"]["messages"].as_array().unwrap().len(),
            1,
            "plain snapshot must retain the active answer; runs={:?}",
            plain["projection"]["runs"]
        );
        let first = client.bounded_thread("t").unwrap();
        assert_eq!(
            first["projection"]["messages"][0]["text"], "The current answer is still visible.",
            "runs={:?}, messages={:?}",
            first["projection"]["runs"], first["projection"]["messages"]
        );
        let visible = first["projection"]["visibleTurnItems"].as_array().unwrap();
        assert_eq!(visible.len(), 75);
        if reposition {
            assert_eq!(visible.last().unwrap()["item"]["id"], "item-answer");
        }
        let mut seen = visible
            .iter()
            .map(|row| row["sourceItemId"].as_str().unwrap().to_owned())
            .collect::<HashSet<_>>();
        let mut cursor = first["historyCursor"].clone();
        while let Some(value) = cursor.as_str() {
            let page = client
                .request_json(
                    "GET",
                    &format!("/api/orchestration/threads/t/history?cursor={value}"),
                    None,
                    true,
                )
                .unwrap();
            for item in page["items"].as_array().unwrap() {
                assert!(
                    seen.insert(item["sourceItemId"].as_str().unwrap().to_owned()),
                    "history repeats an item"
                );
            }
            cursor = page["nextCursor"].clone();
        }
        assert_eq!(
            seen.len(),
            601,
            "history must retain every command and the answer; unexpected: {:?}",
            seen.iter()
                .filter(|id| id.as_str() != "item-answer" && !id.starts_with("tool-"))
                .collect::<Vec<_>>()
        );
    }
}
