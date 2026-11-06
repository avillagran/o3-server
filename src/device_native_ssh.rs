//! Native SSH device command plans and generation-independent reconnect decisions.
//! This module does not execute SSH or install tools.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanError {
    InvalidConfiguration,
    InvalidOwner,
    InvalidReceipt,
}
impl std::fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "Invalid SSH device host configuration",
            Self::InvalidOwner => "Invalid SSH device owner identity",
            Self::InvalidReceipt => "Invalid SSH device owner receipt",
        })
    }
}
impl std::error::Error for PlanError {}
fn js_space(character: char) -> bool {
    matches!(character,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn string(value: &Value) -> Result<String, PlanError> {
    value
        .as_str()
        .map(|value| value.trim_matches(js_space).to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(PlanError::InvalidConfiguration)
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub kind: String,
    pub id: String,
    pub label: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}
impl Config {
    pub fn decode(value: &Value) -> Result<Self, PlanError> {
        let kind = match value.get("kind") {
            None => "ssh",
            Some(value) => value
                .as_str()
                .filter(|value| *value == "ssh")
                .ok_or(PlanError::InvalidConfiguration)?,
        };
        let id = string(&value["id"])?;
        if id == "local"
            || id.encode_utf16().count() > 128
            || !id.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_alphanumeric() || (index > 0 && b"_-".contains(&byte))
            })
        {
            return Err(PlanError::InvalidConfiguration);
        }
        let target = string(&value["target"])?;
        if target.starts_with('-') || target.chars().any(js_space) {
            return Err(PlanError::InvalidConfiguration);
        }
        let identity_file = value.get("identityFile").map(string).transpose()?;
        let port = value
            .get("port")
            .map(|value| {
                value
                    .as_f64()
                    .filter(|port| {
                        port.is_finite() && port.fract() == 0.0 && (1.0..=65535.0).contains(port)
                    })
                    .map(|port| port as u16)
                    .ok_or(PlanError::InvalidConfiguration)
            })
            .transpose()?;
        Ok(Self {
            kind: kind.into(),
            id,
            label: string(&value["label"])?,
            target,
            identity_file,
            port,
        })
    }
}
pub fn owner(environment_id: Option<&str>, state_dir: &str, host_id: &str) -> String {
    let digest = Sha256::digest(
        format!(
            "{}\0{state_dir}\0{host_id}",
            environment_id.unwrap_or(state_dir)
        )
        .as_bytes(),
    );
    format!("{digest:x}")[..24].into()
}
pub fn quote_remote_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
pub const REMOTE_ENVIRONMENT:&str="export PATH=\"$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH\"\nif [ -z \"$ANDROID_HOME\" ]; then\n  if [ -d \"$HOME/Library/Android/sdk\" ]; then export ANDROID_HOME=\"$HOME/Library/Android/sdk\";\n  elif [ -d \"$HOME/Android/Sdk\" ]; then export ANDROID_HOME=\"$HOME/Android/Sdk\"; fi\nfi\nif [ -n \"$ANDROID_HOME\" ]; then export PATH=\"$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$PATH\"; fi\nif [ -z \"$JAVA_HOME\" ] && ! command -v java >/dev/null 2>&1; then\n  for device_java_home in \"$HOME/.local/opt/android-studio/jbr\" /opt/android-studio/jbr /Applications/Android\\ Studio.app/Contents/jbr \"$HOME/Applications/Android Studio.app/Contents/jbr\"; do\n    if [ -x \"$device_java_home/bin/java\" ]; then export JAVA_HOME=\"$device_java_home\"; break; fi\n  done\nfi\nif [ -n \"$JAVA_HOME\" ]; then export PATH=\"$JAVA_HOME/bin:$PATH\"; fi\n";
pub fn remote_command(script: &str) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        quote_remote_arg(&format!("{REMOTE_ENVIRONMENT}{script}")),
    ]
}
fn base(config: &Config) -> Vec<String> {
    let mut args = vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
    ];
    if let Some(port) = config.port {
        args.extend(["-p".into(), port.to_string()]);
    }
    args
}
fn identity(config: &Config, args: &mut Vec<String>) {
    if let Some(identity) = &config.identity_file {
        args.extend(["-i".into(), identity.clone()]);
    }
}
pub fn bootstrap_argv(config: &Config) -> Vec<String> {
    let mut args = base(config);
    args.extend([
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
    ]);
    identity(config, &mut args);
    args.extend(["--".into(), config.target.clone()]);
    args.extend(remote_command("command -v node >/dev/null 2>&1 || { echo \"Node is missing from the non-interactive SSH PATH\" >&2; exit 1; }; exec node"));
    args
}
pub fn run_argv(
    config: &Config,
    owner: &str,
    command: &str,
    arguments: &[String],
) -> Result<Vec<String>, PlanError> {
    if owner.len() != 24
        || !owner
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(PlanError::InvalidOwner);
    }
    let mut args = base(config);
    args.extend([
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
    ]);
    identity(config, &mut args);
    args.extend(["--".into(), config.target.clone()]);
    let words = std::iter::once(command)
        .chain(arguments.iter().map(String::as_str))
        .map(quote_remote_arg)
        .collect::<Vec<_>>()
        .join(" ");
    args.extend(remote_command(&format!(
        "cd \"$HOME/.t3/device/hosts/{owner}\" && exec {words}"
    )));
    Ok(args)
}
pub fn forward_argv(
    config: &Config,
    hub_local: u16,
    hub_remote: u16,
    daemon: Option<(u16, u16)>,
) -> Vec<String> {
    let mut args = base(config);
    identity(config, &mut args);
    for option in [
        "ExitOnForwardFailure=yes",
        "ControlMaster=no",
        "ControlPath=none",
        "ControlPersist=no",
        "ServerAliveInterval=10",
        "ServerAliveCountMax=3",
    ] {
        args.extend(["-o".into(), option.into()]);
    }
    args.extend([
        "-T".into(),
        "-L".into(),
        format!("127.0.0.1:{hub_local}:127.0.0.1:{hub_remote}"),
    ]);
    if let Some((local, remote)) = daemon {
        args.extend(["-L".into(), format!("127.0.0.1:{local}:127.0.0.1:{remote}")]);
    }
    args.extend([
        config.target.clone(),
        "sh".into(),
        "-c".into(),
        quote_remote_arg("exec cat >/dev/null"),
    ]);
    args
}
#[derive(Clone, Copy, Debug)]
pub enum RemoteMode {
    Probe,
    Start,
    Stop,
}
impl RemoteMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Start => "start",
            Self::Stop => "stop",
        }
    }
}
pub fn verified_remote_script(
    owner: &str,
    mode: RemoteMode,
    receipt: Option<&str>,
) -> Result<String, PlanError> {
    if owner.len() != 24
        || !owner
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(PlanError::InvalidOwner);
    }
    let mut request = json!({"protocol":1,"owner":owner,"mode":mode.label()});
    if matches!(mode, RemoteMode::Stop) {
        let receipt = receipt
            .filter(|receipt| {
                receipt.len() == 48
                    && receipt
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .ok_or(PlanError::InvalidReceipt)?;
        request["receipt"] = json!(receipt);
    }
    Ok(format!(
        "const o3VerifiedRemoteRequest = {request};\n{}",
        include_str!("device_native_remote.cjs")
    ))
}
pub fn failed_command(
    config: &Config,
    code: i32,
    stdout: &str,
    stderr: &str,
) -> crate::device_native_hub::CommandResult {
    static TOKENS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?i)("(?:access_token|bearerToken|credential|pairingToken|token)"\s*:\s*")[^"]+(")"#,
        )
        .expect("native SSH token redaction pattern")
    });
    let redacted = TOKENS.replace_all(stdout, "${1}[redacted]${2}");
    let units = redacted.encode_utf16().collect::<Vec<_>>();
    let stdout = if units.len() > 4000 {
        format!(
            "[truncated]\n{}",
            String::from_utf16_lossy(&units[units.len() - 4000..])
        )
    } else {
        redacted.into_owned()
    };
    let stderr = if !stderr.trim_matches(js_space).is_empty() {
        stderr.trim_matches(js_space).to_owned()
    } else if !stdout.trim_matches(js_space).is_empty() {
        stdout.trim_matches(js_space).to_owned()
    } else {
        format!("SSH command failed for {} (exit {code}).", config.target)
    };
    crate::device_native_hub::CommandResult {
        code,
        stdout,
        stderr,
    }
}
pub fn bootstrap_timeout(mode: &str) -> Duration {
    Duration::from_millis(if matches!(mode, "start" | "agent-start") {
        1_300_000
    } else {
        45_000
    })
}
pub fn retry_bind(attempt: usize, step: &str) -> bool {
    attempt < 2 && matches!(step, "forwarding ports" | "waiting for SSH forward")
}
pub fn next_reconnect_delay(current: Duration) -> Duration {
    (current * 2).min(Duration::from_secs(30))
}
pub fn probe_misses(previous: u8, hub_ok: bool, daemon_ok: bool) -> (u8, bool) {
    let misses = if hub_ok && daemon_ok {
        0
    } else {
        previous.saturating_add(1)
    };
    (misses, misses >= 3)
}
pub fn windows_probe_candidate(exit_code: Option<i64>) -> bool {
    exit_code.is_some_and(|code| code >= 1 && (code <= 254 || code == 9009))
}
pub fn windows_shell_report(stdout: &str) -> bool {
    stdout.split(js_space).any(|part| part == "Windows_NT")
}
pub fn windows_node_report(stdout: &str) -> bool {
    stdout.trim_matches(js_space) == "win32"
}
pub fn windows_error(host_id: &str, step: &str, cause: Value) -> Value {
    json!({"_tag":"DeviceHostError","hostId":host_id,"step":step,"cause":{"_tag":"SshWindowsHostError","cause":cause,"message":"Connected over SSH, but this host runs Windows, and SSH device hosts currently need macOS or Linux. To use this machine's devices, run T3 Code on it and connect to that environment."},"message":format!("Device host {host_id} failed while {step}.")})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_native_ssh_command_owner_and_codec_reference() {
        crate::test_sandbox::assert_isolated();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/device-native-ssh-4288.json"
        ))
        .unwrap();
        assert_eq!(
            REMOTE_ENVIRONMENT,
            fixture["remoteEnvironment"].as_str().unwrap()
        );
        for (index, case) in fixture["configCases"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let actual = Config::decode(&case["input"])
                .map(|config| json!({"value":config}))
                .unwrap_or_else(|_| json!({"invalid":true}));
            assert_eq!(
                actual, case["expected"],
                "config {index}: {}",
                case["input"]
            );
        }
        for case in fixture["quoteCases"].as_array().unwrap() {
            assert_eq!(
                quote_remote_arg(case["input"].as_str().unwrap()),
                case["expected"].as_str().unwrap()
            );
        }
        for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let input = &case["input"];
            let config = Config::decode(&input["config"]).unwrap();
            let expected = &case["expected"];
            let owner = owner(
                input["environmentId"].as_str(),
                input["stateDir"].as_str().unwrap(),
                &config.id,
            );
            for actual in expected["owners"].as_array().unwrap() {
                assert_eq!(&json!(owner), actual, "owner {index}");
            }
            for command in expected["commands"].as_array().unwrap() {
                let args = command["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                let actual = if command["forwarding"] == true {
                    let ports = args
                        .windows(2)
                        .filter(|pair| pair[0] == "-L")
                        .map(|pair| pair[1].split(':').collect::<Vec<_>>())
                        .map(|parts| {
                            (
                                parts[1].parse::<u16>().unwrap(),
                                parts[3].parse::<u16>().unwrap(),
                            )
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(command["stdin"], "pipe");
                    forward_argv(&config, ports[0].0, ports[0].1, ports.get(1).copied())
                } else if command["mode"] != "" {
                    bootstrap_argv(&config)
                } else {
                    let run = &input["run"];
                    run_argv(
                        &config,
                        &owner,
                        run["command"].as_str().unwrap(),
                        &serde_json::from_value::<Vec<String>>(run["args"].clone()).unwrap(),
                    )
                    .unwrap()
                };
                assert_eq!(actual, args, "command case {index}: {command}");
            }
            if let Some(failure) = input.get("runFailure") {
                let actual = failed_command(
                    &config,
                    failure["code"].as_i64().unwrap() as i32,
                    failure["stdout"].as_str().unwrap(),
                    failure["stderr"].as_str().unwrap(),
                );
                assert_eq!(
                    json!({"code":actual.code,"stdout":actual.stdout,"stderr":actual.stderr}),
                    expected["runResult"],
                    "failure case {index}"
                );
            }
            assert_eq!(expected["activeAfterStop"], 0, "case {index}");
        }
    }
    #[test]
    fn reconnect_and_unhealthy_windows_are_bounded() {
        crate::test_sandbox::assert_isolated();
        let mut delay = Duration::from_secs(1);
        for seconds in [1, 2, 4, 8, 16, 30, 30] {
            assert_eq!(delay, Duration::from_secs(seconds));
            delay = next_reconnect_delay(delay);
        }
        for attempt in 0..5 {
            for step in [
                "forwarding ports",
                "waiting for SSH forward",
                "probe",
                "start",
            ] {
                assert_eq!(
                    retry_bind(attempt, step),
                    attempt < 2 && matches!(step, "forwarding ports" | "waiting for SSH forward")
                );
            }
        }
        assert_eq!(probe_misses(2, true, true), (0, false));
        assert_eq!(probe_misses(2, true, false), (3, true));
        assert_eq!(probe_misses(0, false, true), (1, false));
        assert_eq!(
            bootstrap_timeout("agent-start"),
            Duration::from_millis(1_300_000)
        );
        assert_eq!(bootstrap_timeout("probe"), Duration::from_secs(45));
        assert!(!windows_probe_candidate(None));
        assert!(!windows_probe_candidate(Some(0)));
        assert!(windows_probe_candidate(Some(9009)));
        assert!(!windows_probe_candidate(Some(9008)));
        assert!(windows_shell_report("noise\nWindows_NT\r\n"));
        assert!(!windows_shell_report("xWindows_NT"));
        assert!(windows_node_report(" win32\n"));
        assert!(run_argv(
            &Config::decode(&json!({"id":"mini","label":"Private","target":"host"})).unwrap(),
            "../unowned",
            "node",
            &[]
        )
        .is_err());
        assert_eq!(
            windows_error("mini", "probe", Value::Null)["cause"]["_tag"],
            "SshWindowsHostError"
        );
    }
}
