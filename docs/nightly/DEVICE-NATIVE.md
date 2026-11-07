# Native device discovery and hub transport

Source pin: T3 Code nightly 4288, revision
`32a0531748c44b25072d7495ce0b9bd7e9450a87`, with Effect 4.1.2.
The intake manifest records the imported source graph and content hashes.

## Implemented foundation

`device_native_hub` implements the native hub request plans, response decoding,
warning filtering, bounded diagnostics, stopped AVD discovery, incomplete-platform
snapshots, boot failures, stream attachment, shutdown confirmation, and screenshot
query encoding. `device_native_client` executes those operations over HTTP against
an already owned loopback hub. Host owners inject platform availability and their
command runner; construction neither starts a tool nor invokes a shell. HTTP
redirects and proxies are disabled. Remote hosts require an owned local SSH forward.

Native deadlines are retained: listing 40 seconds, boot and iOS attachment three
minutes, shutdown 15 seconds, and screenshots 20 seconds. A listing timeout preserves
available platforms as incomplete; a failed AVD listing retains discovered Android
devices and reports its narrower advisory flag. Valid iOS attachment responses are
accepted even when their `ok` value is false, matching the source adapter. An iOS
shutdown error is accepted only when a subsequent listing confirms that device is
off. Screenshots return the hub's original bytes, without pretending to validate
an image format.

`device_native_state` implements complete-platform refresh merging, host-generation
checks, exact session snapshot keys, preservation of sessions opened or reopened
while discovery is in flight, diagnostic restoration, boot-stage publication and
cleanup, postboot identity confirmation, and session replacement. Incomplete
platforms retain their previous devices. A returned or original device identity
confirms boot only when that platform's fresh listing is complete.

## Evidence

The two generator tools execute complete pinned native modules, rather than copies
of the Rust implementation. The service generator imports the full static source
graph and uses native Effect scheduling, deferred concurrency, pub/sub and test
clock behavior. Its only additional pinned dependency is `diff` 8.0.3.

The checked-in corpora cover:

- 458 native listing outcomes, 66 boot/attachment/shutdown outcomes, and 14 binary
  screenshot/status/transport outcomes.
- 80 service refresh scenarios, including support disabled, host replacement,
  concurrent new sessions, and reopened sessions while discovery is suspended.
- 128 status/diagnostic transitions and 64 boot/session lifecycle scenarios.

The physical HTTP test replays all 538 hub vectors through an actual private TCP
server and `reqwest`. It checks request methods, paths, query encoding, payloads,
command plans, bytes, typed results, and native error projections. HTTP exception
stacks are runtime-specific and excluded from comparison; actual operation errors
retain a cause, and boot failures retain the native structured cause. Test-only
short deadlines exercise timeout branches without waiting several minutes.

All executable proofs run in rootless Podman with read-only source, private HOME
and temporary directories, and no external network. Host tools are injected mocks;
no ADB, SDK, simulator, emulator, desktop display, hardware, npm installer or actual
SSH service is invoked. The source oracle's dependency acquisition used a separate
private container with lifecycle scripts disabled.

## Owned local runtime and media consumers

A manifest-verified installation at the profile's
`tools/expo-device-hub/0.19.1` enables the real local owner. It launches the
resolved Node executable with argv and an owned process group, waits for
`/readyz` for at most 30 seconds, and closes the process scope after startup
failure, cancellation, explicit stop or owner release. Crash supervision checks
its generation before and after the native restart delay: the first replacement
is immediate, repeated short lifetimes back off to 30 seconds, and a stable
minute resets the delay. Stale or foreign PID records are not adopted or killed.

The platform adapter retains native SDK search ordering, explicit-root precedence,
PATH/real-path fallback, platform diagnostics, old-emulator camera overrides, and
owner-checked Linux runtime-directory discovery. The source corpus adds 480 SDK
and host-environment cases and 432 camera/runtime-directory cases. Android virtual
device sessions require an ffmpeg/libx264 check before opening.

`device.rs` now uses the owned native runtime for inventory, boot, attachment,
session creation, shutdown and disabled-support teardown when a verified tool is
present. The connected ADB implementation remains available when no native
installation exists. Incomplete or invalid native installations produce errors;
they are not executed or silently replaced. Native boot retains its three-minute
deadline rather than the old ADB request budget.

The authenticated `/api/device-hub` branch forwards only to the current owner's
verified loopback origin. It applies the native route and method allowlists,
strips credentials and routing tickets, rewrites Origin, and rejects redirects.
HTTP bodies are streamed with bounded buffering, a 1 MiB request limit, 8 MiB
ordinary-response limit, 64 MiB screenshot limit, and 20-second upstream idle
budget. Continuous MJPEG/AVCC/event streams have no total-size limit. Screenshot
responses use `image/png`. Upstream cookies are discarded. WebSocket forwarding
bounds frames/messages at 16 MiB, connects within ten seconds, and checks device
settings and session revocation every two seconds. Existing operate-scope admission
remains in place; this checkpoint does not relax the old HTTP authorization rule.
Shutdown also terminates media forwarding.

The full native proxy router generated 816 outcomes spanning 34 paths, six methods
and four scope sets. Rust compares route decisions, query stripping and forwarded
headers; tracing IDs are excluded because Effect generates them at runtime.
A physical synthetic-child test exercises the actual device RPC consumer, boot
confirmation, session lifecycle, bearer-authenticated screenshot HTTP, credential
stripping, bidirectional WebSocket forwarding, blocked shell paths, and disabled
support stopping/reaping its owned process. The child is a private Python HTTP/
WebSocket fixture; it is not an emulator or a downloaded package.

## Tool acquisition boundary and remaining work

`device_native_tools` validates pinned package metadata and a complete SHA-256
installation inventory. Entry presence alone does not prove installation. It
rejects links, special files, extra or changed files, and oversized inventories;
file reads use `O_NOFOLLOW`. Sealing uses create-new manifest semantics within an
exclusively owned staging directory. The npm acquisition plan pins exact package
versions, disables lifecycle scripts and bin links, and records a package lock.
No installation tree is recursively removed. The Linux acquisition/update consumer now publishes through owned staging and
atomic no-replace rename, retaining failed artifacts in quarantine. See
[Safe pinned device tool acquisition](DEVICE-TOOL-ACQUISITION.md) for its separate
synthetic download/process proof and upstream-provenance limits. These original
runtime tests did not invoke a real npm installation.

SSH bootstrap/forward ownership, agent-device daemon ownership, persistent crash
recovery, EAS, Simlock and physical hardware acceptance remain incomplete. There is
no claim that iOS or Android hardware ran, that packages were installed, or that
all panel actions now work. Native iOS settings/actions and device automation still
need their consumers. Tool-update, EAS and Simlock capability flags remain false.
No managed-policy family is enabled by this work.

## Validation commands

The final local-runtime checkpoint passes all 15 device unit tests in both minimal
and default feature modes, including the real private child/RPC/HTTP/WebSocket
consumer test. Strict all-target Clippy passes in both modes. Formatting uses the
repository's Rust 2021 edition.

```sh
tools/test-scheduled-modes --compile-lock /var/tmp/o3-build/o3-parity-compile.lock --unit-filter device_native
tools/test-scheduled-modes --compile-lock /var/tmp/o3-build/o3-parity-compile.lock --default-features --unit-filter device_native
```
