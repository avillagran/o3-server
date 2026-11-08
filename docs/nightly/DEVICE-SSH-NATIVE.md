# SSH device ownership foundation

Source pin: T3 Code nightly 4288,
`32a0531748c44b25072d7495ce0b9bd7e9450a87`, with Effect 4.1.2.
The verified-installed SSH owner is registered by the real device runtime.
Readiness and media routing require its current owned loopback forward; missing
or invalid installations cannot start a hub or create a session. Remote tool
installation, agent-device runtime, EAS and Simlock remain unavailable. Local
selected-tool acquisition is documented separately in
[Safe pinned device tool acquisition](DEVICE-TOOL-ACQUISITION.md).

## Source-backed command and lifecycle decisions

The reference generator imports the complete native `SshDeviceHost` factory and
its static dependency graph. Fake Effect child handles intercept every SSH
invocation and consume bootstrap scripts as text; those scripts are never
executed. Fake port reservation and HTTP services prevent host/network actions.

The corpus contains 297 factory scenarios covering platform-specific executable
selection, identity-file quoting, configured ports, environment-ID fallback,
owner derivation, zero through three forward failures, agent endpoint shapes,
remote command argv, and failed-command diagnostic redaction/tail handling.
It also contains 138 actual contract codec outcomes and native shell quoting and
environment strings. Rust compares the tested argv, owner, codec, quoting and
failed-command projections directly; the complete factory traces are retained
for review. Executable selection is captured in the native trace; the transport
receives its resolved executable from its caller.

The implementation preserves owner derivation from
`environmentId + NUL + stateDir + NUL + hostId`, truncated to 24 hexadecimal
SHA-256 characters; the exact remote shell quoting; explicit batch-mode command
argv; nonmultiplexed loopback forwards; an owned stdin lifetime pipe; three
fresh-port attempts for forward/readiness failures; and separate SSH reconnect
backoff starting at one second and capped at 30 seconds. Forward readiness uses
15 seconds. Health checks run every ten seconds with a five-second budget and
require three consecutive misses before reconnecting. A successful probe resets
the miss counter. Local process-group ownership closes the forward after stop,
cancellation, failure or owner release, even if a caller retains a ready handle.

`device_native_ssh_transport` executes argv without a shell, bounds command
output, and accepts only matching owner/protocol metadata, a valid remote port,
an absolute Node path and a validated receipt. The hub's local origin comes from
the owned loopback forward. `device_native_ssh_owner` serializes starts/stops,
checks generations before reconnecting, retains a receipt on teardown failure,
and stops the local forward before requesting remote stop. Negative platform
reports are reprobed; positive reports are cached as in the native adapter.

## Verified-installed remote lifecycle

The native remote installer performs recursive removal. It is not copied or run.
The new remote lifecycle core accepts only an existing pinned
`expo-device-hub@0.19.1` tree sealed by the O3 complete-inventory manifest. It
verifies package metadata and every regular file with bounded streaming SHA-256,
rejects links/special files/changed or extra entries, uses `O_NOFOLLOW` and
nonblocking file opens, and never downloads, extracts or recursively removes a
tree. Missing or invalid tools are reported rather than installed.

Startup records only the current owner's process, port, entry and random receipt
in a separate O3 record slot. Stop requires the acquired receipt, a matching
owner/protocol record, and live process metadata containing the entry, port,
owner and nonce. An unknown process or changed receipt remains untouched. Missing
records cannot report a confirmed stop. Owned stop still works if the installed
package changes after launch, because stopping does not execute that package.
Retired records are retained by rename. Unknown lock files are refused and never
automatically cleared; recovery of stale locks remains explicit future work.

The runtime receipt is an ownership discriminator, not an SSH credential. Process
environments are examined only in memory for ownership verification and are not
returned or logged. No credential files, secret stores or native agent claim
slots are read, written or removed.

The safe remote core deliberately differs from the native installer: it never
installs tools, does not adopt native hub records, and does not manage an
agent-device daemon. It returns its own protocol/receipt metadata. No claim of
complete remote installer compatibility is made. Callers must await `stop()` for
remote teardown; dropping the owner closes its local forward while retaining the
remote process record for explicit receipt-based teardown or recovery.

## Validation

The six focused owner tests pass in both minimal and default feature modes.
The registered consumers add private unit coverage, including the production
worker-stack flow and admission-lease lifetime. Strict Clippy is checked in both
modes. The tests cover the complete native corpus plus twelve
injected remote-filesystem/process scenarios and real private CLI forwarding:

- Read-only verified probing, healthy reuse, stale receipts, changed packages,
  owner mismatches, failed startup, missing/foreign records, and unknown locks.
- Link fixtures target only a sibling directory inside a private temporary tree.
- Actual private TCP forwarding, two failed bind attempts, shared concurrent
  startup, hub HTTP, exact-receipt teardown, three-miss health reconnect, and
  local owner release with a retained ready handle.

The SSH-shaped CLI is a private Python TCP peer that never executes stdin or
invokes SSH. The remote lifecycle test injects process/SDK/HTTP operations and
never starts a tool. All execution occurs in rootless Podman, or the repository's
verified GitHub Docker CI environment, with read-only source, private HOME/tmp and
no external network. No remote host, SSH authentication, SDK, emulator, display,
agent daemon or hardware was exercised. Tests do not remove trees.

```sh
tools/test-scheduled-modes --compile-lock /var/tmp/o3-build/o3-parity-compile.lock --integration device_native_ssh
tools/test-scheduled-modes --compile-lock /var/tmp/o3-build/o3-parity-compile.lock --default-features --integration device_native_ssh
```

## Runtime registration and protocol consumers

The device runtime decodes configured `deviceHosts` and constructs SSH owners
without network actions while support is disabled. Enabled listing uses the
source's four-host concurrency limit, merges local and SSH inventories/sessions,
and preserves per-platform incomplete-discovery behavior. A read-only
`inspectOnly` request takes precedence when no tool update is requested; it probes
metadata without starting, reconciling, replacing or stopping a connection.

An existing host must confirm receipt-based remote stop before its registration
can be replaced or removed. Failures retain its registration and receipt, close
its local forward, remain retryable, and surface failed metadata to subscribers.
A changed configuration cannot route media to that retained connection. No
cleanup error is converted into successful teardown. Unverified remote tools
produce failed metadata and cannot forward traffic or create sessions. A verified
SSH host remains available when the local host has no supported SDK.

The real protocol2 WebSocket dispatcher exercises list, read-only host testing,
configure, open and close. Authenticated HTTP screenshots route by `hostId` to the
current owner; caller URLs are never accepted as an upstream. The proxy retains
the HTTP connection/peer admission lease after WebSocket upgrade and releases it
when the media connection ends. The admission-lifetime test uses an actual private
upgrade, echo stream and semaphore.

The complete wire RPC/session/media flow passes on a measured 524,288-byte Tokio
worker stack, matching the production worker budget. Profile fixture preparation
runs before spawning the consumer task, matching production startup. No stack
limit is raised. See `DEVICE-STACK-BOUNDS.md` for evidence, the bounded-buffer fix,
and the limits of the diagnosis.

Remote tool acquisition/quarantine, stale-lock recovery, agent lifecycle and
credentials, native iOS/SSH settings and automation actions, and real hardware
acceptance remain separate consumers. EAS, Simlock and managed-policy family
readiness are unchanged. No actual SSH host, SDK or device was used.
