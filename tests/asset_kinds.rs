mod common;
use common::*;
use serde_json::json;

#[test]
fn native_icon_tool_image_and_credentialed_media_use_signed_urls() {
    let data = std::env::temp_dir().join(format!("o3-icons-{}", std::process::id()));
    std::fs::create_dir_all(data.join("icons/hicolor/48x48/apps")).unwrap();
    // This integration-test executable contains one test; set before spawning any server threads.
    unsafe { std::env::set_var("XDG_DATA_HOME", &data) };
    write(
        &data.join("icons/hicolor/48x48/apps/o3-proof.png"),
        b"\x89PNG\r\n\x1a\n",
    );
    let e = setup("asset-kinds");
    let root = e.dir("workspace");
    e.project("p", &root);
    e.thread("t", "p");
    let icon=e.conn.rpc("assets.createUrl",json!({"resource":{"_tag":"native-app-icon","app":{"_tag":"app-id","appId":"o3-proof"}}})).unwrap();
    assert_eq!(
        http(
            &e.base,
            "GET",
            icon["relativeUrl"].as_str().unwrap(),
            &[],
            b""
        )
        .2,
        b"\x89PNG\r\n\x1a\n"
    );
    e.db().execute("INSERT INTO records VALUES('turnItems','image','t',?)",[json!({"id":"image","threadId":"t","output":{"images":[{"mimeType":"image/png","data":"iVBORw0KGgo="}]}}).to_string()]).unwrap();
    let image=e.conn.rpc("assets.createUrl",json!({"resource":{"_tag":"tool-output-image","threadId":"t","itemId":"image","index":0}})).unwrap();
    assert_eq!(
        http(
            &e.base,
            "GET",
            image["relativeUrl"].as_str().unwrap(),
            &[],
            b""
        )
        .2,
        b"\x89PNG\r\n\x1a\n"
    );
    let gh = bin_dir().join("gh");
    std::fs::write(&gh, "#!/bin/sh\nprintf '\\211PNG\\r\\n\\032\\n'\n").unwrap();
    let media=e.conn.rpc("assets.createUrl",json!({"resource":{"_tag":"github-media","cwd":root,"url":"https://github.com/user-attachments/assets/proof"}})).unwrap();
    assert_eq!(
        http(
            &e.base,
            "GET",
            media["relativeUrl"].as_str().unwrap(),
            &[],
            b""
        )
        .2,
        b"\x89PNG\r\n\x1a\n"
    );
    assert!(e
        .conn
        .rpc(
            "assets.createUrl",
            json!({"resource":{"_tag":"github-media","cwd":root,"url":"https://localhost/private"}})
        )
        .is_err());
    std::fs::remove_dir_all(data).unwrap();
}
