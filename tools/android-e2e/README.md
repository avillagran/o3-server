# Android regression helper

Run from the repository root. The helper is deliberately resumable: it never silently
replaces an existing server/emulator. It fixes the SDK device to `emulator-5570`, port
38900, release executable `/var/tmp/o3-build/main/release/o3-server`, profile
`/var/tmp/o3-build/android/profile`, and evidence directory under that same root.
It does not call T3 device tools or access public routes. Do not use a physical phone.

Prerequisites: Node with WebSocket support, Python, installed SDK Android 34 Google
APIs x86_64 image, KVM, `unshare` user/mount namespaces, debug O3 APK, authenticated
Claude/Codex CLIs. Put runnable provider executables in `$W/bin`. The runner prepends
that directory to PATH. Java builds require Java 21 and an explicit disk temporary
directory (`JAVA_TOOL_OPTIONS=-Djava.io.tmpdir=/var/tmp/o3-build/android/tmp`).

```sh
export W=/var/tmp/o3-build/android
mkdir -p "$W"/tmp /var/tmp/o3-build/tmp
TMPDIR=/var/tmp/o3-build/tmp CARGO_TARGET_DIR=/var/tmp/o3-build/main nice cargo build --release --locked -j 4
tools/android-e2e/run avd       # fresh AVD only; never recreate while running
tools/android-e2e/run emulator  # private mount namespace confines SDK crashpad temporaries
tools/android-e2e/run adb wait-for-device
# Wait until this returns 1, and package manager is available:
tools/android-e2e/run adb shell getprop sys.boot_completed
tools/android-e2e/run adb install -r ~/Work/ot3-android/app/build/outputs/apk/debug/app-debug.apk
tools/android-e2e/run adb reverse tcp:38900 tcp:38900
tools/android-e2e/run auth      # copies only provider auth into private test home; issues private session
tools/android-e2e/run server
mkdir -p "$W/repo"
git -C "$W/repo" init
node tools/android-e2e/seed.mjs --session-file "$W/session" "$W/repo"
```

Connect the app to `http://127.0.0.1:38900` with the private session bearer, or use
`run pair` for a short-lived pairing credential stored in `$W/pairing`. Never print,
commit or screenshot credentials. Use `run adb shell input ...` for UI operations;
`tools/android-e2e/type 'ASCII text'` waits between characters for Compose updates.
Wait for an attachment chip before sending. Use `run capture <name>` to save XML,
PNG and observation time; the XML output prints text/description/bounds to locate
controls. Captures are actual emulator pixels, not synthesized renderings.

Create fresh Claude Haiku and Codex threads in-app, leave the title empty, send a
short prompt, and save `01-haiku`, `03-codex`. Save RPC projections with:

```sh
node tools/android-e2e/rpc.mjs orchestration.subscribeShell '{}' "$W/evidence/shell.json"
node tools/android-e2e/rpc.mjs orchestration.getThreadProjection '{"threadId":"<id>"}' "$W/evidence/01-haiku-rpc.json"
```

The RPC helper reads `$W/session`, implements protocol 2 heartbeat/acks, and exits
nonzero on RPC failure/timeout. Projections must have completed runs and nonstreaming
assistant messages; UI text alone is insufficient. Verify Claude's title is a summary;
Codex's prompt fallback is expected. In Haiku Supervised mode request a Write, capture
`04-pending` and its projection, approve in-app, capture `04-approved`, its projection
and compare the exact file bytes on disk. Push a synthetic blue PNG to Downloads,
select it in-app, wait for the chip, send a color question, capture `05-image-pass`
and its projection (user attachment present, assistant Blue). Open the app terminal,
type `echo O3_TERMINAL_OK`, capture `06-terminal`, and use `terminal.observe` with
`threadId` and `terminalId: "term-1"` for its snapshot/history.

`run hwm` records `/proc/PID/status`, excluding provider children. `run sample` samples for five minutes (250 ms)
across server restarts; start it before inference and retain maximum observed value across server lifetimes.
`run stop-server` signals only the process group launched by this helper. Record
wall time, capture offline state (server port must refuse connections), restart using
`run server`, then wait through reconnect backoff and capture Connected. Send another
prompt in the same thread and assert a new completed run in its projection. Keep actual
observation times; a requested 2s delay plus UI dump does not mean a 2s detection time.

Finally run `tools/android-e2e/verify` before cleanup. It checks this run's fixed evidence
names and emits a sanitized report; missing evidence fails, never defaults to PASS.
`run cleanup` removes reverse, clears only the emulator app, stops the isolated server,
shuts the designated emulator down, and deletes copied credentials/profile. Retain
raw evidence locally; publish only synthetic screenshots and the sanitized report.
