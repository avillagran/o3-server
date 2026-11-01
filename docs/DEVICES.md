# Connected Android devices

The local implementation uses installed `adb` with argv subprocesses, a ten-second
limit and an 8 MiB output cap. It discovers only authorized, already-connected
devices. It never launches emulators, boots or shuts down targets, or installs tools.
The environment descriptor advertises `capabilities.devices` only when adb resolves.

RPCs return the desktop's host/device/session shapes. Configure persists flags;
sessions are bounded and in memory. Detail provides identity and a read timestamp;
settings and foreground application inspection remain unavailable. Launch/terminate
package actions are supported; other action variants return typed errors. SSH and
EAS return explicit unavailability without starting processes or storing tokens.

Each device-state subscription owns a bounded latest-value channel and a cancellable
two-second discovery task. Dropping it aborts its work; no background discovery is
created at server startup or left behind after the last subscription closes.

The hub accepts media tickets or bearer credentials, checks read/operate scopes,
and supports Android POST screenshot and an input WebSocket (keys and tap on touch
release). All other paths return explicit unavailability. No H264 encoder is started.
**Desktop limitation:** device_screen.rs currently uses PNG fallback only when its
video decoder is absent. With a decoder installed, its panel receives no video.
Touch drags, text, multitouch, iOS and full device-hub API parity are not implemented.

`tests/device.rs` uses the real pinned t3-protocol client and a fake adb on PATH;
it covers discovery, configure, open/detail/close, authenticated PNG/input,
EAS refusal and stopping discovery on unsubscribe. No physical hardware or desktop
UI was exercised.
