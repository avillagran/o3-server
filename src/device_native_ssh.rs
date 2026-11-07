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
