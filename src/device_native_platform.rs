//! Native local SDK/platform discovery and argv-only host command adapter.
use crate::device_native_client::HostTools;
use crate::device_native_hub::{CommandResult, Platform};
use crate::device_native_owner::HostFuture;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AndroidSdk {
    pub root: Option<PathBuf>,
    pub adb: bool,
    pub emulator: bool,
    pub avdmanager: bool,
    pub legacy_avdmanager: bool,
}
fn trimmed(environment: &BTreeMap<String, String>, key: &str) -> Option<String> {
    environment
        .get(key)
        .map(|value| crate::native_goal::trim(value))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
pub fn android_sdk(
    environment: &BTreeMap<String, String>,
    platform: &str,
    exists: impl Fn(&Path) -> bool,
    real_path: impl Fn(&Path) -> PathBuf,
) -> AndroidSdk {
    let windows = platform == "win32";
    let home = environment
        .get("HOME")
        .or_else(|| environment.get("USERPROFILE"))
        .map(String::as_str)
        .unwrap_or("");
    let explicit =
        trimmed(environment, "ANDROID_HOME").or_else(|| trimmed(environment, "ANDROID_SDK_ROOT"));
    let adb_name = if windows { "adb.exe" } else { "adb" };
    let avd_name = if windows {
        "avdmanager.bat"
    } else {
        "avdmanager"
    };
    let inspect = |root: PathBuf| -> Option<AndroidSdk> {
        let adb = exists(&root.join("platform-tools").join(adb_name));
        let emulator =
            exists(
                &root
                    .join("emulator")
                    .join(if windows { "emulator.exe" } else { "emulator" }),
            );
        if explicit.is_none() && !adb && !emulator {
            return None;
        }
        let avdmanager = exists(&root.join("cmdline-tools/latest/bin").join(avd_name));
        let legacy_avdmanager = !avdmanager && exists(&root.join("tools/bin").join(avd_name));
        Some(AndroidSdk {
            root: Some(root),
            adb,
            emulator,
            avdmanager,
            legacy_avdmanager,
        })
    };
    let candidates = if let Some(explicit) = &explicit {
        vec![PathBuf::from(explicit)]
    } else {
        vec![
            Path::new(home).join("Library/Android/sdk"),
            Path::new(home).join("Android/Sdk"),
            environment
                .get("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| Path::new(home).join("AppData/Local"))
                .join("Android/Sdk"),
        ]
    };
    for root in candidates {
        if let Some(sdk) = inspect(root) {
            return sdk;
        }
    }
    if explicit.is_none() {
        for directory in environment
            .get("PATH")
            .or_else(|| environment.get("Path"))
            .map(String::as_str)
            .unwrap_or("")
            .split(if windows { ';' } else { ':' })
        {
            if directory.is_empty() {
                continue;
            }
            let adb = Path::new(directory).join(adb_name);
            if !exists(&adb) {
                continue;
            }
            let real = real_path(&adb);
            let root = real
                .parent()
                .and_then(Path::parent)
                .unwrap_or(Path::new("."));
            if let Some(sdk) = inspect(root.to_owned()) {
                return sdk;
            }
        }
    }
    AndroidSdk::default()
}
pub fn platform_reason(
    platform: Platform,
    host: &str,
    sdk: &AndroidSdk,
    xcrun: bool,
) -> Option<String> {
    if platform == Platform::Ios {
        return if host != "darwin" {
            Some("iOS Simulators need macOS with Xcode.".into())
        } else if !xcrun {
            Some("Xcode command line tools were not found.".into())
        } else {
            None
        };
    }
    let Some(root) = &sdk.root else {
        return Some("Android SDK was not found. Install it with Android Studio or set ANDROID_HOME to your SDK directory.".into());
    };
    let root = root.display();
    if !sdk.adb {
        return Some(format!("Android SDK Platform-Tools are missing from {root}. Install them in Android Studio's SDK Manager."));
    }
    if !sdk.emulator {
        return Some(format!(
            "Android Emulator is missing from {root}. Install it in Android Studio's SDK Manager."
        ));
    }
    if !sdk.avdmanager {
        return Some(if sdk.legacy_avdmanager {
            format!("The Android SDK command-line tools in {root} appear to be an older, unsupported version. Install Android SDK Command-line Tools (latest) in Android Studio's SDK Manager under SDK Tools.")
        } else {
            format!("Android SDK Command-line Tools (latest) are missing from {root}. Install them in Android Studio's SDK Manager.")
        });
    }
    None
}
pub fn host_environment(
    environment: &BTreeMap<String, String>,
    sdk: &AndroidSdk,
    platform: &str,
) -> BTreeMap<String, String> {
    let mut result = environment.clone();
    if let Some(root) = &sdk.root {
        result.insert("ANDROID_HOME".into(), root.to_string_lossy().into_owned());
        result.insert(
            "PATH".into(),
            [
                root.join("platform-tools").to_string_lossy().into_owned(),
                root.join("emulator").to_string_lossy().into_owned(),
                environment
                    .get("PATH")
                    .or_else(|| environment.get("Path"))
                    .cloned()
                    .unwrap_or_default(),
            ]
            .join(if platform == "win32" { ";" } else { ":" }),
        );
    }
    result
}
fn ascii_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
fn contains_js_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(start, _)| {
        !start
            .checked_sub(1)
            .and_then(|index| text.as_bytes().get(index))
            .is_some_and(|byte| ascii_word(*byte))
            && !text
                .as_bytes()
                .get(start + word.len())
                .is_some_and(|byte| ascii_word(*byte))
    })
}
pub fn hub_environment(
    environment: &BTreeMap<String, String>,
    platform: &str,
    uid: Option<u32>,
    valid_runtime_directory: bool,
    cameras: [Option<&CommandResult>; 2],
) -> BTreeMap<String, String> {
    let mut result = environment.clone();
    result.insert("FORCE_COLOR".into(), "0".into());
    result.insert("NO_COLOR".into(), "1".into());
    if environment
        .get("ANDROID_HOME")
        .is_some_and(|root| !root.is_empty())
    {
        let mut overrides = Vec::new();
        for (index, result) in cameras.into_iter().enumerate() {
            if let Some(result) = result {
                let help = format!("{}\n{}", result.stdout, result.stderr);
                if result.code == 0
                    && contains_js_word(&help, "emulated")
                    && !contains_js_word(&help, "imagefile")
                {
                    overrides.push(format!(
                        "-camera-{} emulated",
                        if index == 0 { "front" } else { "back" }
                    ));
                }
            }
        }
        if !overrides.is_empty() {
            let mut value = overrides.join(" ");
            if let Some(extra) = environment
                .get("EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS")
                .filter(|extra| !extra.is_empty())
            {
                value.push(' ');
                value.push_str(extra);
            }
            result.insert("EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS".into(), value);
        }
    }
    if platform == "linux"
        && !environment.contains_key("XDG_RUNTIME_DIR")
        && valid_runtime_directory
    {
        if let Some(uid) = uid {
            result.insert("XDG_RUNTIME_DIR".into(), format!("/run/user/{uid}"));
        }
    }
    result
}
/// Commands are resolved from this host's discovered SDK, never concatenated into a shell.
pub struct LocalTools {
    pub sdk: AndroidSdk,
    pub platform: String,
    pub environment: BTreeMap<String, String>,
    pub cwd: PathBuf,
}
impl LocalTools {
    pub fn discover(environment: BTreeMap<String, String>, cwd: PathBuf) -> Self {
        let platform = if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(windows) {
            "win32"
        } else {
            "linux"
        }
        .to_string();
        let sdk = android_sdk(&environment, &platform, Path::exists, |path| {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
        });
        let environment = host_environment(&environment, &sdk, &platform);
        Self {
            sdk,
            platform,
            environment,
            cwd,
        }
    }
    pub fn availability_reason(&self, platform: Platform) -> Option<String> {
        platform_reason(
            platform,
            &self.platform,
            &self.sdk,
            crate::executable::resolve(Path::new("xcrun")).is_some(),
        )
    }
}
impl HostTools for LocalTools {
    fn video_encoder(&self) -> HostFuture<'_, Result<(), serde_json::Value>> {
        Box::pin(async move {
            let program =
                trimmed(&self.environment, "SERVE_EMU_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
            let env = self
                .environment
                .iter()
                .map(|(key, value)| (key.as_str(), Path::new(value)))
                .collect::<Vec<_>>();
            let out = crate::proc::run(
                &program,
                &self.cwd,
                &["-hide_banner", "-encoders"],
                crate::proc::Opts {
                    timeout: Duration::from_secs(10),
                    cap: 4 * 1024 * 1024,
                    allow_truncate: false,
                },
                &env,
            )
            .await;
            let out = out
                .ok()
                .filter(|out| out.ok())
                .ok_or_else(|| crate::device_native_client::video_encoder_error(false))?;
            let text = format!("{}\n{}", out.text(), out.stderr);
            let pattern = regex::Regex::new(r"(?m)^\s*V\S*[ \t]+libx264(?:[ \t]|$)")
                .expect("static encoder pattern");
            if pattern.is_match(&text) {
                Ok(())
            } else {
                Err(crate::device_native_client::video_encoder_error(true))
            }
        })
    }
    fn platform_available(&self, platform: Platform) -> HostFuture<'_, bool> {
        Box::pin(async move { self.availability_reason(platform).is_none() })
    }
    fn run<'a>(&'a self, command: &'a str, args: &'a [String]) -> HostFuture<'a, CommandResult> {
        Box::pin(async move {
            let program = match command {
                "emulator" => self
                    .sdk
                    .root
                    .as_ref()
                    .filter(|_| self.sdk.emulator)
                    .map(|root| {
                        root.join("emulator").join(if self.platform == "win32" {
                            "emulator.exe"
                        } else {
                            "emulator"
                        })
                    }),
                _ => None,
            };
            let Some(program) = program else {
                return CommandResult {
                    code: 127,
                    stdout: String::new(),
                    stderr: "Device command is unavailable on this host".into(),
                };
            };
            let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
            let env = self
                .environment
                .iter()
                .map(|(key, value)| (key.as_str(), Path::new(value)))
                .collect::<Vec<_>>();
            match crate::proc::run(
                &program.to_string_lossy(),
                &self.cwd,
                &refs,
                crate::proc::Opts {
                    timeout: Duration::from_secs(20),
                    cap: 4 * 1024 * 1024,
                    allow_truncate: false,
                },
                &env,
            )
            .await
            {
                Ok(out) => CommandResult {
                    code: out.code.unwrap_or(127),
                    stdout: out.text(),
                    stderr: out.stderr,
                },
                Err(error) => CommandResult {
                    code: if matches!(error, crate::proc::RunError::Timeout(_)) {
                        124
                    } else {
                        127
                    },
                    stdout: String::new(),
                    stderr: format!("{error:?}"),
                },
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    #[test]
    fn complete_native_local_platform_helpers() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-platform-4288.json"
        ))
        .unwrap();
        for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let input = &case["input"];
            let environment: BTreeMap<String, String> =
                serde_json::from_value(input["environment"].clone()).unwrap();
            let platform = input["platform"].as_str().unwrap();
            let sdk = android_sdk(
                &environment,
                platform,
                |path| {
                    input["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|file| file == &json!(path.to_string_lossy()))
                },
                |path| {
                    input["realpaths"][path.to_str().unwrap()]
                        .as_str()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| path.to_owned())
                },
            );
            let actual = json!({"sdk":{"root":sdk.root,"adb":sdk.adb,"emulator":sdk.emulator,"avdmanager":sdk.avdmanager,"legacyAvdmanager":sdk.legacy_avdmanager},"reason":platform_reason(Platform::Android,platform,&sdk,false),"environment":host_environment(&environment,&sdk,platform)});
            assert_eq!(actual, case["expected"], "case {index}: {input}");
        }
    }
    #[test]
    fn complete_native_hub_environment_helpers() {
        crate::test_sandbox::assert_isolated();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-platform-4288.json"
        ))
        .unwrap();
        for (index, case) in fixture["hubCases"].as_array().unwrap().iter().enumerate() {
            let input = &case["input"];
            let environment = serde_json::from_value(input["environment"].clone()).unwrap();
            let cameras: [CommandResult; 2] =
                serde_json::from_value(input["cameras"].clone()).unwrap();
            let actual = hub_environment(
                &environment,
                input["platform"].as_str().unwrap(),
                input["uid"].as_u64().map(|uid| uid as u32),
                input["validRuntimeDirectory"] == true,
                [Some(&cameras[0]), Some(&cameras[1])],
            );
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                case["expected"],
                "case {index}: {input}"
            );
        }
    }
}
