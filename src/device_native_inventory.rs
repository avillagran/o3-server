//! Read-only native tool version decisions backed by verified O3 installation manifests.
use crate::device_native_tools::{InstalledTool, Tool};
use serde_json::{json, Value};
use std::{fs, io, path::Path, sync::OnceLock};
#[derive(Clone, Debug)]
pub enum Read<T> {
    Value(T),
    Missing,
    Failed,
}
#[derive(Clone, Debug)]
pub struct Version {
    pub name: String,
    pub sentinel: Read<String>,
    pub entry: Read<bool>,
}
pub fn valid_name(value: &str) -> bool {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            regex::Regex::new(r"\A[0-9]+\.[0-9]+\.[0-9]+(?:-[a-zA-Z0-9.-]+)?\z")
                .expect("fixed native version expression")
        })
        .is_match(value)
}
fn group(tool: Tool, rows: Read<Vec<Version>>, running: Option<&str>) -> Option<Value> {
    let rows = match rows {
        Read::Value(rows) => rows,
        Read::Missing => vec![],
        Read::Failed => return None,
    };
    let mut installed = Vec::new();
    for row in rows {
        if !valid_name(&row.name) {
            continue;
        }
        let sentinel = match row.sentinel {
            Read::Value(value) => value,
            Read::Missing => continue,
            Read::Failed => return None,
        };
        if crate::native_goal::trim(&sentinel) != row.name {
            continue;
        }
        match row.entry {
            Read::Value(true) => installed.push(row.name),
            Read::Value(false) | Read::Missing => {}
            Read::Failed => return None,
        }
    }
    installed.sort();
    Some(
        json!({"requiredVersion":tool.version(),"installedVersions":installed,"runningVersion":running}),
    )
}
pub fn versions(
    hub: Read<Vec<Version>>,
    agent: Read<Vec<Version>>,
    running_hub: Option<&str>,
    running_agent: Option<&str>,
) -> Option<Value> {
    Some(
        json!({"hub":group(Tool::Hub,hub,running_hub)?,"agent":group(Tool::Agent,agent,running_agent)?}),
    )
}
pub struct Inspected {
    pub tools: Value,
    pub qualification: Option<&'static str>,
}
fn verified_rows(
    base: &Path,
    tool: Tool,
    qualification: &mut Option<&'static str>,
) -> io::Result<Read<Vec<Version>>> {
    let directory = base.join("tools").join(tool.package());
    match crate::device_native_tools::real_directory(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Read::Missing),
        Err(error) => return Err(error),
    }
    let mut rows = Vec::new();
    let mut count = 0;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        count += 1;
        if count > 1024 {
            return Err(io::Error::other("Tool inventory exceeds bounded entries"));
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !valid_name(name) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::other("Tool version path is unsafe"));
        }
        match InstalledTool::inspect_version(&entry.path(), tool, name) {
            Ok(_) => rows.push(Version {
                name: name.into(),
                sentinel: Read::Value(name.into()),
                entry: Read::Value(true),
            }),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                *qualification=Some("Some local device tool versions could not be verified. Existing files have not been changed.");
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Read::Value(rows))
}
/// Complete historical manifests are inventory metadata; execution still uses fixed-pin inspect.
pub fn inspect(base: &Path, running_hub: Option<&str>) -> io::Result<Inspected> {
    let mut qualification = None;
    let hub = verified_rows(base, Tool::Hub, &mut qualification)?;
    let agent = verified_rows(base, Tool::Agent, &mut qualification)?;
    let tools = versions(hub, agent, running_hub, None)
        .ok_or_else(|| io::Error::other("Tool inventory is unavailable"))?;
    Ok(Inspected {
        tools,
        qualification,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn rows(input: &Value, tool: &str) -> Read<Vec<Version>> {
        match input["directoryErrors"][tool].as_str() {
            Some("NotFound") => return Read::Missing,
            Some(_) => return Read::Failed,
            None => {}
        }
        Read::Value(
            input["entries"][tool]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| Version {
                    name: row["version"].as_str().unwrap().into(),
                    sentinel: if row["missing"] == true {
                        Read::Missing
                    } else if row["sentinelError"].is_string() {
                        Read::Failed
                    } else {
                        Read::Value(
                            row.get("sentinel")
                                .and_then(Value::as_str)
                                .unwrap_or(row["version"].as_str().unwrap())
                                .into(),
                        )
                    },
                    entry: if row["entryError"].is_string() {
                        Read::Failed
                    } else {
                        Read::Value(row["entry"] != false)
                    },
                })
                .collect(),
        )
    }
    #[test]
    fn native_inventory_corpus_matches_all_version_sentinel_sorting_and_failure_outcomes() {
        crate::test_sandbox::assert_isolated();
        let corpus: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-inventory-4288.json"
        ))
        .unwrap();
        assert_eq!(corpus["cases"].as_array().unwrap().len(), 295);
        for case in corpus["cases"].as_array().unwrap() {
            let input = &case["input"];
            let actual = versions(
                rows(input, "hub"),
                rows(input, "agent"),
                input["running"]["hub"].as_str(),
                input["running"]["agent"].as_str(),
            );
            assert_eq!(actual, case["expected"].get("value").cloned(), "{input}");
        }
    }
    #[test]
    fn native_decoded_authorization_and_dispatch_rows_match_effective_permissions() {
        crate::test_sandbox::assert_isolated();
        let corpus: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-inventory-4288.json"
        ))
        .unwrap();
        assert_eq!(corpus["authCases"].as_array().unwrap().len(), 23);
        for case in corpus["authCases"].as_array().unwrap() {
            let input = &case["input"];
            let action = crate::device::list_action(input);
            if case["invalid"] == true {
                assert!(action.is_err(), "{input}");
                continue;
            }
            let action = action.unwrap();
            let expected = &case["expected"];
            match action {
                crate::device::ListAction::List => assert_eq!(expected["action"], "list"),
                crate::device::ListAction::Inspect => assert_eq!(expected["action"], "inspect"),
                crate::device::ListAction::Retry(host) => {
                    assert_eq!(expected["action"], "retry");
                    assert_eq!(expected["host"], host);
                }
                crate::device::ListAction::Update(tool) => {
                    assert_eq!(expected["action"], "update");
                    assert_eq!(
                        expected["tool"],
                        if tool.package() == "expo-device-hub" {
                            "hub"
                        } else {
                            "agent"
                        }
                    );
                }
            }
            for scopes in [
                vec![],
                vec!["orchestration:read"],
                vec!["orchestration:operate"],
                vec!["orchestration:read", "settings:write"],
                vec!["orchestration:read", "orchestration:operate"],
            ] {
                let required = case["baseScope"].as_str().unwrap();
                let extra = expected["extraScope"].as_str();
                let permitted =
                    scopes.contains(&required) && extra.is_none_or(|scope| scopes.contains(&scope));
                assert_eq!(
                    crate::rpc::authorize(&json!({"scopes":scopes}), "device.list", input).is_ok(),
                    permitted,
                    "{input}"
                );
            }
        }
    }
    fn fixture() -> std::path::PathBuf {
        crate::test_sandbox::assert_isolated();
        let root = std::env::temp_dir().join(format!(
            "device-inventory-{}",
            crate::store::random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
    fn completed(base: &Path, tool: Tool, version: &str) -> std::path::PathBuf {
        use sha2::Digest;
        let root = base.join("tools").join(tool.package()).join(version);
        let package = root.join("node_modules").join(tool.package());
        let entry = package.join(tool.entry());
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        fs::write(
            package.join("package.json"),
            json!({"name":tool.package(),"version":tool.version()}).to_string(),
        )
        .unwrap();
        fs::write(&entry, "// Private synthetic tool; never executed").unwrap();
        InstalledTool::seal_staging(&root, tool).unwrap();
        let bytes = json!({"name":tool.package(),"version":version}).to_string();
        fs::write(package.join("package.json"), &bytes).unwrap();
        let manifest_path = root.join(".o3-device-install.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["version"] = json!(version);
        manifest["files"][format!("node_modules/{}/package.json", tool.package())] =
            json!(format!("{:x}", sha2::Sha256::digest(bytes.as_bytes())));
        fs::write(manifest_path, manifest.to_string()).unwrap();
        root
    }
    #[test]
    fn historical_complete_manifests_are_inventory_only_and_execution_keeps_fixed_pin() {
        let base = fixture();
        let empty = inspect(&base, None).unwrap();
        assert_eq!(empty.tools["hub"]["installedVersions"], json!([]));
        assert!(!base.join("tools").exists());
        for version in ["2.0.0", "10.0.0", "0.19.1", "0.18.0"] {
            completed(&base, Tool::Hub, version);
        }
        completed(&base, Tool::Agent, "0.21.31");
        let inventory = inspect(&base, Some("0.19.1")).unwrap();
        assert!(inventory.qualification.is_none());
        assert_eq!(
            inventory.tools["hub"]["installedVersions"],
            json!(["0.18.0", "0.19.1", "10.0.0", "2.0.0"])
        );
        assert_eq!(inventory.tools["hub"]["runningVersion"], "0.19.1");
        assert_eq!(inventory.tools["agent"]["runningVersion"], Value::Null);
        let old = base.join("tools/expo-device-hub/0.18.0");
        InstalledTool::inspect_version(&old, Tool::Hub, "0.18.0").unwrap();
        assert!(InstalledTool::inspect(&old, Tool::Hub).is_err());
    }
    #[test]
    fn sentinel_only_and_modified_trees_are_qualified_preserved_and_never_adopted() {
        let base = fixture();
        let good = completed(&base, Tool::Hub, "0.19.1");
        let unknown = base.join("tools/expo-device-hub/9.9.9");
        fs::create_dir(&unknown).unwrap();
        fs::write(unknown.join(".install-complete"), "9.9.9\n").unwrap();
        fs::write(unknown.join("unknown-data"), "retained").unwrap();
        let result = inspect(&base, None).unwrap();
        assert!(result.qualification.is_some());
        assert_eq!(result.tools["hub"]["installedVersions"], json!(["0.19.1"]));
        assert_eq!(
            fs::read_to_string(unknown.join("unknown-data")).unwrap(),
            "retained"
        );
        assert!(!unknown.join(".o3-device-install.json").exists());
        fs::write(
            good.join("node_modules/expo-device-hub/dist/server/cli.mjs"),
            "changed fixture",
        )
        .unwrap();
        let result = inspect(&base, None).unwrap();
        assert!(result.qualification.is_some());
        assert_eq!(result.tools["hub"]["installedVersions"], json!([]));
        assert!(unknown.join(".install-complete").is_file());
        assert_eq!(
            fs::read_to_string(good.join("node_modules/expo-device-hub/dist/server/cli.mjs"))
                .unwrap(),
            "changed fixture"
        );
    }
    #[test]
    fn ancestor_and_version_links_are_refused_even_when_the_expected_child_is_missing() {
        let root = fixture();
        let target = root.join("private-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("kept"), "untouched").unwrap();
        let alias = root.join("profile-alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        assert!(inspect(&alias, None).is_err());
        let profile = root.join("profile");
        fs::create_dir(&profile).unwrap();
        std::os::unix::fs::symlink(&target, profile.join("tools")).unwrap();
        assert!(inspect(&profile, None).is_err());
        let profile = root.join("version-profile");
        let tools = profile.join("tools/expo-device-hub");
        fs::create_dir_all(&tools).unwrap();
        std::os::unix::fs::symlink(&target, tools.join("0.19.1")).unwrap();
        assert!(inspect(&profile, None).is_err());
        assert_eq!(
            fs::read_to_string(target.join("kept")).unwrap(),
            "untouched"
        );
    }
}
