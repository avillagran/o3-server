//! Differential structure fixtures from nightly 4165, plus the actual isolated CLI.
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
};
fn run(path: &Path, ext: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_o3-server"))
        .args(["__document-text"])
        .arg(path)
        .arg(ext)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
#[test]
fn nightly_office_epub_fixtures_keep_order_structure_and_bounds() {
    let root = std::env::temp_dir().join(format!("o3-document-fixtures-{}", std::process::id()));
    // Fixture trees are retained for private container teardown.
    std::fs::create_dir_all(&root).unwrap();
    let mut json = String::new();
    flate2::read::GzDecoder::new(
        &include_bytes!("fixtures/document-text/nightly-cases.json.gz")[..],
    )
    .read_to_string(&mut json)
    .unwrap();
    let cases: Vec<Value> = serde_json::from_str(&json).unwrap();
    for (i, case) in cases.iter().enumerate() {
        let ext = case["ext"].as_str().unwrap();
        let path = root.join(format!("case-{i}.{ext}"));
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, content) in case["parts"].as_object().unwrap() {
            let bytes = if let Some(s) = content.as_str() {
                s.as_bytes().to_vec()
            } else {
                content["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u8)
                    .collect()
            };
            let method = if name.ends_with(".bmp") || name.ends_with(".bin") {
                zip::CompressionMethod::Deflated
            } else {
                zip::CompressionMethod::Stored
            };
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap();
        let result = run(&path, ext);
        assert!(
            result.status.success(),
            "case {i} {ext}: {:?}",
            result.status
        );
        let text = String::from_utf8(result.stdout).unwrap();
        let mut previous = None;
        for expected in case["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
        {
            let index = text.find(expected).unwrap_or_else(|| {
                panic!(
                    "case {i} {ext}: missing {expected:?} in {:?}",
                    &text[..text.len().min(600)]
                )
            });
            assert!(previous.is_none_or(|p| index > p), "case {i}: order");
            previous = Some(index);
        }
        for absent in case["absent"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| v.as_str().unwrap())
        {
            assert!(!text.contains(absent), "case {i}: unexpected {absent:?}");
        }
        if case.get("occurrences").is_some() {
            assert_eq!(
                text.matches(case["occurrences"]["text"].as_str().unwrap())
                    .count() as u64,
                case["occurrences"]["count"].as_u64().unwrap(),
                "case {i}"
            )
        }
        assert!(text.encode_utf16().count() < 1_000_100, "case {i}");
        assert!(
            text.matches("[Extracted text truncated").count() <= 1,
            "case {i}"
        );
    }
    // Fixture trees are retained for private container teardown.
}
#[test]
fn rtf_notebook_worker_has_no_profile_effect_and_keeps_unicode() {
    let root = std::env::temp_dir().join(format!("o3-document-plain-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let rtf = root.join("symbols.rtf");
    for (input, expected) in [
        (
            r"{\rtf1\ansi Pages 1\endash 5\emdash\lquote left\rquote\ldblquote right\rdblquote\bullet\b End\b0\unknown{\*\secret\bullet hidden}}",
            "Pages 1–5—‘left’“right”•End",
        ),
        (
            r"{\rtf1\ansi\uc10\u945\endash\emdash\enspace\emspace\qmspace\lquote\rquote\ldblquote\rdblquote\bullet Z}",
            "αZ",
        ),
        (r"{\rtf1\ansi\uc3\u945\~\_\-Z}", "αZ"),
        (r"{\rtf1\ansi A\u9\tab X}", "A\tX"),
        (r"{\rtf1\ansi{\u945}Z}", "αZ"),
        (r"{\rtf1\ansi\u945{Z}}", "αZ"),
        (r"{\rtf1\ansi\u945\'3fZ}", "αZ"),
        (r"{\rtf1\ansi\u945\{Z}", "αZ"),
        (r"{\rtf1\ansi\u945\*Z}", "αZ"),
        (r"{\rtf1\ansi\uc4\u945?\tab\'3f\~Z}", "αZ"),
        (r"{\rtf1\ansi\u945\bin3 abcZ}", "αZ"),
        (r"{\rtf1\ansi\uc0\u945Z}", "αZ"),
        (r"{\rtf1\ansi{\uc2\u945??}\u946?Z}", "αβZ"),
        (r"{\rtf1\ansi\u-10179?\u-8704?}", "😀"),
    ] {
        std::fs::write(&rtf, input).unwrap();
        let v = run(&rtf, "rtf");
        assert!(v.status.success(), "{input}");
        assert_eq!(String::from_utf8(v.stdout).unwrap(), expected, "{input}");
    }
    std::fs::write(&rtf, r"{\rtf1\ansicpg932 unsupported}").unwrap();
    assert_eq!(run(&rtf, "rtf").status.code(), Some(2));
    let book = root.join("test.ipynb");
    std::fs::write(&book,serde_json::json!({"cells":[{"cell_type":"markdown","source":["# Report\n"]},{"cell_type":"code","source":"1+1","outputs":[{"data":{"text/plain":["2"],"image/png":"DO-NOT-PRINT"}},{"output_type":"error","ename":"ValueError","evalue":"bad","traceback":["\u{1b}[31mboom\u{1b}[0m"]}]}]}).to_string()).unwrap();
    let v = run(&book, "ipynb");
    assert!(v.status.success());
    let text = String::from_utf8(v.stdout).unwrap();
    assert!(text.contains("## Cell 1 (markdown)\n# Report"));
    assert!(text.contains("ValueError: bad\nboom"));
    assert!(!text.contains("DO-NOT-PRINT") && !text.contains('\u{1b}'));
    assert!(!root.join("o3.sqlite").exists());
    // Fixture trees are retained for private container teardown.
}
#[test]
fn archive_count_expansion_and_raw_output_are_bounded() {
    let root = std::env::temp_dir().join(format!("o3-document-limits-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("limits.docx");
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer
        .write_all(b"<document><p>Kept</p></document>")
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let mut wrong_count = bytes.clone();
    let end = wrong_count
        .windows(4)
        .rposition(|p| p == b"PK\x05\x06")
        .unwrap();
    wrong_count[end + 8..end + 12].copy_from_slice(&[1, 8, 1, 8]);
    std::fs::write(&path, wrong_count).unwrap();
    let result = run(&path, "docx");
    assert!(result.status.success());
    assert!(String::from_utf8(result.stdout)
        .unwrap()
        .contains("[Extracted text truncated"));
    let mut huge_entry = bytes.clone();
    let central = huge_entry
        .windows(4)
        .position(|p| p == b"PK\x01\x02")
        .unwrap();
    huge_entry[central + 24..central + 28].copy_from_slice(&(33u32 * 1024 * 1024).to_le_bytes());
    std::fs::write(&path, huge_entry).unwrap();
    let result = run(&path, "docx");
    assert!(result.status.success());
    assert!(String::from_utf8(result.stdout)
        .unwrap()
        .contains("[Extracted text truncated"));
    let mut bomb = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    bomb.start_file(
        "word/document.xml",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated),
    )
    .unwrap();
    bomb.write_all(format!("<document><p>{}</p></document>", "x".repeat(100_000)).as_bytes())
        .unwrap();
    bomb.finish().unwrap();
    let result = run(&path, "docx");
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        "[Extracted text truncated at document extraction limits.]"
    );
    let rtf = root.join("spaces.rtf");
    std::fs::write(
        &rtf,
        format!("{{\\rtf1 First{}Later}}", " ".repeat(1_000_000)),
    )
    .unwrap();
    let result = run(&rtf, "rtf");
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "First\n\n[Extracted text truncated at document extraction limits.]"
    );
    std::fs::File::create(&rtf)
        .unwrap()
        .set_len(17 * 1024 * 1024)
        .unwrap();
    let result = run(&rtf, "rtf");
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        "[Extracted text truncated at document extraction limits.]"
    );
    // Fixture trees are retained for private container teardown.
}
