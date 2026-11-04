mod common;
use common::*;
use serde_json::{json, Value};

fn search(e: &Env, q: &str, extra: Value) -> Value {
    let mut p = json!({"cwd":e.scratch.join("workspace"),"query":q,"limit":100,"caseSensitive":true,"wholeWord":false,"useRegex":false});
    p.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    e.conn.rpc("projects.searchContents", p).unwrap()
}
fn workspace(e: &Env) -> std::path::PathBuf {
    let p = e.scratch.join("workspace");
    std::fs::create_dir_all(&p).unwrap();
    p
}
#[test]
fn content_search_matches_nightly_ranges_words_and_options() {
    let e = setup("content-search");
    let w = workspace(&e);
    std::fs::write(
        w.join("words.txt"),
        "𐐀foo foo foo𐐀\n-foo- -foo-\nSquare\nsquare\n spaced \n",
    )
    .unwrap();
    let r = search(&e, "foo", json!({"wholeWord":true}));
    assert_eq!(r["matches"][0]["matchRanges"], json!([{"start":6,"end":9}]));
    let r = search(&e, "-foo-", json!({"wholeWord":true}));
    assert_eq!(
        r["matches"][0]["matchRanges"],
        json!([{"start":0,"end":5},{"start":6,"end":11}])
    );
    std::fs::write(w.join("edges.txt"), "a-foo-b\n").unwrap();
    let edge = search(&e, "-foo-", json!({"wholeWord":true}));
    assert!(edge["matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["path"] == "edges.txt"));
    assert_eq!(
        search(&e, "Square", json!({}))["matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        search(
            &e,
            "\\SQUARE",
            json!({"useRegex":true,"caseSensitive":false})
        )["matches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        search(&e, " spaced ", json!({}))["matches"][0]["lineNumber"],
        5
    );
}
#[test]
fn content_search_respects_ignore_binary_links_limits_and_fresh_edits() {
    let e = setup("content-search-ignore");
    let w = workspace(&e);
    std::process::Command::new("git")
        .arg("init")
        .arg(&w)
        .output()
        .unwrap();
    std::fs::write(w.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(w.join("ignored.txt"), "needle").unwrap();
    std::fs::write(w.join("binary"), b"needle\0needle").unwrap();
    std::fs::write(w.join("dense.txt"), "needle\n".repeat(300)).unwrap();
    std::fs::write(w.join("other.txt"), "needle\n").unwrap();
    let outside = e.scratch.join("outside");
    std::fs::write(&outside, "needle\n").unwrap();
    std::os::unix::fs::symlink(&outside, w.join("link.txt")).unwrap();
    let r = search(&e, "needle", json!({"limit":500}));
    assert_eq!(r["matches"].as_array().unwrap().len(), 101, "{r}");
    assert!(r["matches"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["path"] == "dense.txt" || m["path"] == "other.txt"));
    let r = search(&e, "needle", json!({"limit":1}));
    assert_eq!(r["matches"].as_array().unwrap().len(), 1);
    assert_eq!(r["truncated"], true);
    std::fs::write(w.join("other.txt"), "brandnew\n").unwrap();
    assert_eq!(
        search(&e, "brandnew", json!({}))["matches"][0]["path"],
        "other.txt"
    );
    std::fs::write(w.join("literal.txt"), "[\n").unwrap();
    let r = search(&e, "[", json!({"useRegex":true}));
    assert_eq!(r["matches"][0]["path"], "literal.txt");
    assert!(r["regexFallbackError"].is_string());
    let err = e.conn.rpc("projects.searchContents",json!({"cwd":w.join("absent"),"query":"x","limit":1,"caseSensitive":true,"wholeWord":false,"useRegex":false})).unwrap_err();
    assert!(err.to_string().contains("No such file"), "{err}");
}

#[test]
fn content_search_bounds_resources_and_filters_before_counting() {
    let e = setup("content-search-bounds");
    let w = workspace(&e);
    std::fs::write(w.join("late.txt"), format!("{}foo\n", "afoo\n".repeat(10))).unwrap();
    let r = search(&e, "foo", json!({"wholeWord":true,"limit":1}));
    assert_eq!(r["matches"][0]["lineNumber"], 11);
    assert_eq!(r["truncated"], false);
    std::fs::write(w.join("large.txt"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    std::fs::write(w.join("long.txt"), format!("{}\nfoo\n", "x".repeat(65537))).unwrap();
    let r = search(&e, "foo", json!({"wholeWord":true}));
    assert_eq!(r["truncated"], true);
    assert!(r["matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["path"] == "long.txt" && m["lineNumber"] == 2));
}
