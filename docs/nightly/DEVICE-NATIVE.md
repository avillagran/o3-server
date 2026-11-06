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

## Remaining integration

The existing `device.rs` consumer remains unchanged. These modules are a verified
foundation, not an assertion that the side panel now owns a full device host.
Local and SSH hub ownership, pinned tool acquisition, agent-device daemon ownership,
restart supervision, forwarding, device RPC integration and panel streaming still
need their real consumers and acceptance proofs. EAS, Simlock and physical hardware
support are not advertised by this checkpoint. No managed-policy family is enabled
by this work.
