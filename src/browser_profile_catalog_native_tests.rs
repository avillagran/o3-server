//! Differential cases against original-generated references. Register this
//! module only after the native oracle has run and its scalar fixture is sealed.
use super::*;
use std::collections::BTreeMap;

#[test]
fn complete_native_catalog_traces_and_scalar_schema_cases_match() {
    crate::test_sandbox::assert_isolated();
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/browser-profile-catalog-native-scalar-4314.json"
    ))
    .unwrap();
    assert_eq!(fixture["complete"], true);
    assert_eq!(
        fixture["revision"],
        "83a3674ffd84ffb33de768210c806cb1ac53fd80"
    );
    let proof = fixture["originalProofSha256"].as_str().unwrap();
    assert_eq!(proof.len(), 64);
    assert!(proof
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    assert_eq!(
        fixture["excludedNativeSchemaCases"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut snapshots = 0;
    for trace in fixture["traces"].as_array().unwrap() {
        let catalog = Catalog::default();
        let mut tokens = BTreeMap::new();
        for row in trace["rows"].as_array().unwrap() {
            let step = row["step"].as_array().unwrap();
            match step[0].as_str().unwrap() {
                "initial" => {}
                "acquire" => {
                    assert!(tokens
                        .insert(
                            step[1].as_str().unwrap().to_owned(),
                            catalog.reporter().unwrap()
                        )
                        .is_none());
                }
                "report" => tokens
                    .get(step[1].as_str().unwrap())
                    .unwrap()
                    .report(Report::decode(&step[2]).unwrap())
                    .unwrap(),
                "retire" => {
                    assert!(tokens.remove(step[1].as_str().unwrap()).is_some());
                }
                other => panic!("Unrecognized native fixture step: {other}"),
            }
            let snapshot = catalog.snapshot();
            assert_eq!(
                snapshot.status(),
                row["status"],
                "Native trace {} step {}",
                trace["name"],
                row["step"]
            );
            assert_eq!(
                json!(snapshot.recording_frame_rate),
                row["recordingFrameRate"]
            );
            snapshots += 1;
        }
        assert!(tokens.is_empty());
    }
    assert_eq!(snapshots, 26);
    let cases = fixture["schemaCases"].as_array().unwrap();
    assert_eq!(cases.len(), 18);
    for case in cases {
        let decoded = Report::decode(&case["input"]);
        if case["accepted"] == true {
            assert_eq!(decoded.unwrap().value(), case["decoded"]);
        } else {
            assert!(
                decoded.is_err(),
                "Native rejected profile report was accepted: {}",
                case["input"]
            );
        }
    }
}
