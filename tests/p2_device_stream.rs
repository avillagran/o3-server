mod support;
use std::{os::unix::fs::PermissionsExt, time::Duration};
use t3_protocol::{
    api::device,
    device_hub::{self, Platform},
};
#[test]
fn native_h264_packets_rotation_and_shutdown_use_fixture_adb() {
    let s = support::start(|_| {});
    let bin = s.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/device/screen.h264");
    let pid = s.dir.join("recorder.pid");
    let log = s.dir.join("adb.log");
    std::fs::write(
        bin.join("adb"),
        format!(
            r#"#!/usr/bin/python3
import sys,os,time,json
args=sys.argv[1:]
with open({log:?},'a') as f:f.write(json.dumps(args)+'\n')
if args[-1:] == ['get-state']: print('device')
elif args[-2:] == ['wm','size']: print('Physical size: 1080x1920')
elif 'screenrecord' in args:
 with open({pid:?},'w') as f:f.write(str(os.getpid()))
 sys.stdout.buffer.write(open({fixture:?},'rb').read());sys.stdout.buffer.flush()
 time.sleep(30)
"#,
            log = log.to_str().unwrap(),
            pid = pid.to_str().unwrap(),
            fixture = fixture.to_str().unwrap()
        ),
    )
    .unwrap();
    std::fs::set_permissions(bin.join("adb"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let old = std::env::var_os("PATH").unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), old.to_string_lossy()),
    );
    let c = s.connect();
    device::configure(&c, Some(true), Some(true), None).unwrap();
    device::open(&c, "t", "emulator-fixture", "android", None, Some(false)).unwrap();
    let access = c.device_hub("/api/device-hub", "local").unwrap();
    let mut ws = c
        .device_socket(&access, Platform::Android, "emulator-fixture")
        .unwrap();
    ws.send(
        device_hub::input_message(Platform::Android, &serde_json::json!({"type":"keyframe"}))
            .unwrap(),
    )
    .unwrap();
    let packet = ws.read().unwrap();
    let data = packet.into_data();
    let (_, payload) = device_hub::parse_semu(&data).expect("real SEMU reader");
    assert_eq!(payload, std::fs::read(fixture).unwrap());
    // The same Annex B bytes fed to the desktop decoder produce a real frame.
    use std::io::Write;
    let mut decoder = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-threads",
            "1",
            "-f",
            "h264",
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-threads",
            "1",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    decoder.stdin.take().unwrap().write_all(payload).unwrap();
    let decoded = decoder.wait_with_output().unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(decoded.stdout.len(), 32 * 32 * 3);

    ws.close(None).unwrap();
    drop(ws);
    let child = std::fs::read_to_string(pid).unwrap();
    assert!(support::wait_for(Duration::from_secs(3), || {
        !std::path::Path::new(&format!("/proc/{}", child.trim())).exists()
    }));
    device::action(
        &c,
        &device::DeviceAction {
            device_id: "emulator-fixture".into(),
            host_id: None,
            action: device::Action::SetOrientation("landscape_left".into()),
        },
    )
    .unwrap();
    device::close(&c, "t", None, Some("emulator-fixture"), Some(true)).unwrap();
    let argv = std::fs::read_to_string(log).unwrap();
    assert!(argv.contains("acceleration"), "{argv}");
    assert!(argv.contains("kill"), "{argv}");
    std::env::set_var("PATH", old);
}
