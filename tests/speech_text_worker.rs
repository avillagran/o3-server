//! Whole native text helpers in an actual owned Node process, without an ASR engine.
#[path = "support/sandbox.rs"]
mod sandbox;
use o3_server::{
    speech_kernel,
    speech_request::TranscriptionOptions,
    speech_worker::{Cleanup, NodeRuntime, SealedFile, TextWorkerSpec, Worker},
};
use serde_json::Value;
use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
fn private() -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "speech-text-{}",
        o3_server::store::random_id().unwrap()
    ));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    directory
}
#[tokio::test]
async fn original_cloud_finalization_and_utf16_prompt_need_no_model_or_engine_authority() {
    sandbox::assert_isolated();
    let directory = private();
    let source = PathBuf::from(std::env::var("O3_ADMITTED_NODE").unwrap());
    let node = NodeRuntime::capture(
        SealedFile::admit(
            &source,
            &std::env::var("O3_ADMITTED_NODE_SHA256").unwrap(),
            fs::metadata(&source).unwrap().len(),
        )
        .unwrap(),
        &directory.join("node"),
    )
    .unwrap();
    let (_cancel, stop) = tokio::sync::watch::channel(false);
    let worker = Worker::start_text(
        TextWorkerSpec {
            node,
            directory: directory.join("run"),
            startup_deadline: Duration::from_secs(10),
            operation_deadline: Duration::from_secs(5),
            close_deadline: Duration::from_secs(2),
        },
        stop,
    )
    .await
    .unwrap();
    assert!(!directory.join("run/engine").exists());
    assert!(!directory.join("run/model").exists());
    assert!(!directory.join("run/vad").exists());
    assert!(
        worker.load().await.is_err(),
        "Text owner cannot obtain ASR import authority"
    );
    let cloud: Value =
        serde_json::from_str(include_str!("fixtures/cloud-speech-4314.json")).unwrap();
    let model = speech_kernel::ModelLanguages {
        languages: cloud["cloudSpeechLanguages"]["languages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect(),
        supports_language_detection: true,
    };
    let mut matched = 0;
    for row in cloud["cases"].as_array().unwrap() {
        let commands = row["commands"].as_array().unwrap();
        if commands.len() != 1 {
            continue;
        }
        let Some(expected) = row["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["label"] == "transcribe" && step["ok"] == true)
        else {
            continue;
        };
        let command = &commands[0];
        let options = TranscriptionOptions::decode(command["options"].clone()).unwrap();
        let language = speech_kernel::effective_language(
            &model,
            options.as_value()["speechLanguage"].as_str().unwrap(),
        );
        let raw = row["input"]["reply"]["text"]
            .as_str()
            .unwrap_or("  um, oh three  ");
        assert_eq!(
            worker
                .correct_cloud(raw, &options, &language)
                .await
                .unwrap(),
            expected["value"],
            "{}",
            row["name"]
        );
        matched += 1;
    }
    assert!(
        matched >= 5,
        "Require genuine successful whole-CloudSpeech traces"
    );
    let post: Value =
        serde_json::from_str(include_str!("fixtures/speech-postprocess-4314.json")).unwrap();
    assert_eq!(post["promptCases"].as_array().unwrap().len(), 12);
    for row in post["promptCases"].as_array().unwrap() {
        let actual = worker
            .postprocess_prompt(
                "fixture instructions",
                "fixture transcript",
                &row["draft"],
                row["correction"].as_str().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), row["resultUnits"]);
    }
    for row in post["cases"].as_array().unwrap() {
        if row["outcome"]["ok"] != true
            || !row["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == "generation")
        {
            continue;
        }
        let input = &row["input"];
        let result = worker
            .correct_postprocess(
                row["generated"].as_str().unwrap(),
                input["transcript"].as_str().unwrap(),
                &input["options"]["speechCustomWords"],
            )
            .await
            .unwrap();
        assert_eq!(result, row["outcome"]["value"]["text"], "{}", row["name"]);
    }
    assert_eq!(worker.close().await, Cleanup::Confirmed { graceful: true });
    assert_eq!(worker.close().await, Cleanup::Confirmed { graceful: true });
    // All fixture resources are retained; no tree deletion is used for cleanup.
    assert!(directory.join("run/spec.json").is_file());
}
