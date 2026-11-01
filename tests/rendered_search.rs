mod common;
use common::*;
use serde_json::json;

#[test]
fn search_uses_rendered_blocks_skills_context_and_folded_answers() {
    let e = setup("rendered-search");
    e.project("p", &e.dir("workspace"));
    e.thread("t", "p");
    let rows = [
        json!({"id":"a","messageId":"a","type":"user_message","text":"Check **the build** [docs](https://hidden.example)\n\nUse $test-app now [context](t3-context:file/ref)","runId":null}),
        json!({"id":"b","messageId":"async-answer:req","type":"user_message","text":"excluded answer","runId":null}),
        json!({"id":"q","type":"user_input_request","questionAnswer":{"requestId":"req"},"runId":null}),
    ];
    for (position, item) in rows.iter().enumerate() {
        e.db()
            .execute(
                "INSERT INTO records(kind,id,thread_id,body) VALUES('visibleTurnItems',?,'t',?)",
                rusqlite::params![
                    item["id"].as_str().unwrap(),
                    json!({"position":position,"item":item}).to_string()
                ],
            )
            .unwrap();
    }
    let find = |query| {
        e.conn.rpc("orchestration.searchThread",json!({"threadId":"t","query":query,"skills":[{"name":"test-app","displayName":"App Testing"}]})).unwrap()
    };
    for query in ["check the build", "App Testing"] {
        assert_eq!(find(query)["totalMatches"], 1, "{query}");
    }
    for query in [
        "hidden.example",
        "context",
        "excluded answer",
        "docs Use",
        "$test-app",
    ] {
        assert_eq!(find(query)["totalMatches"], 0, "{query}");
    }
}
