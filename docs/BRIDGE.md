# Embedded OmaBridge

The default `bridge` Cargo feature runs OmaBridge on o3-server's two-worker Tokio runtime.
`--no-default-features` removes it. No bridge executable or service is started by the server.

OmaBridge is pinned to pushed `feat/embed` revision
`0fc1610ec6c96a76a293108f6b8c6737266c7ebc`. There is no local path patch.
The real protocol client and bridge both use the pinned `t3-protocol` revision
`8c80c4dc62414c454eaefb4706d973adf8d8ee74`: linking different revisions together fails
because each library exports the same C ABI symbols. The newer local protocol checkout
also removed `bridge_catalog`, so it cannot replace the bridge's pinned version.

## Stages from the embedding research §4.5

- M1: superseded by the direct transport; no intermediate loopback server is shipped.
- M2: `PairingIssuer` is explicit, with a SQLite-backed o3 implementation. The legacy
  `T3Issuer` and constructors retain CLI discovery for standalone users. Embedded mode
  never detects a T3 CLI. Pairings last two minutes and retain the authenticated peer's
  `omabridge:` label through session exchange. CLI session list/revoke supports the
  desktop's grant-withdrawal cleanup.
- M3: `Target::Local(Arc<dyn LocalService>)` supplies a 32 KiB duplex to `serve_io`.
  `Target::Tcp` remains for standalone services. Authorization, public consent,
  compression and the one-second grant recheck remain in OmaBridge. HTTP and WS still
  require their own session credentials.
- M4: `ServeBackend`, `EmbeddedBackend` and `UnitsBackend` separate publication from
  systemd. Embedded publish/unpublish writes private atomic `services.json`; each new
  stream and each active stream's grant recheck consult it, so unpublish cuts streams.
  Remote management retains the existing admin checks and audit. An embedded Manager
  uses only explicit state paths. Standalone CLI management recognizes embedded state
  and routes publication to its file backend. In-process backend restart cancels service
  sessions and keeps the endpoint identity and control socket; it does not rebind QUIC.
- M5: the server reads `Store::service_routes` directly, verifies candidate descriptors
  against its environment id, and caches `kind: omabridge` direct endpoints in config.
  A node-owned task refreshes every 30 seconds (up to eight routes, two seconds each),
  runs `Publisher::publish_auto` without a desktop process, and is aborted on shutdown.
  Routes expire and consent filtering remains in the bridge store. There are no
  advertised gateway endpoints without a valid, descriptor-verified HTTPS route.

## Runtime and state

```
o3-server serve --base-dir /absolute/isolated-profile --port 0
```

Bridge state defaults to `<base-dir>/omabridge`, deliberately separate from an installed
bridge's identity. Set `--bridge-state-dir /absolute/path` to reuse an explicitly chosen
identity after migration. The runtime lock rejects a second host of that identity.
This change performs no service migration, systemd operation or default bridge-state write.

`--bridge-bind IP:PORT` and `--bridge-relay none|public|HTTPS_URL` select networking.
The safe development defaults are `127.0.0.1:0` and no relay. For LAN/NAT deployments,
choose an appropriate bind and relay explicitly. Only direct loopback QUIC was measured.
Embedded `NodeConfig` enables the control socket and peer sync. Its explicit
`standalone_supervision: false` prevents the legacy systemd relay rollback path.
Gateway and relay hosting remain disabled unless requested. Explicit pairing/hello/control operations remain available.
The control socket bind must succeed and its mode is 0600.

## Desktop compatibility and required changes

The wire protocol of `peers`, `forward`, `connect`, `t3-pair` and remote `manage` is retained.
The existing client-side `omabridge` CLI may still be used; the server does not need it.

The desktop branch `feat/o3-server` now detects `<T3CODE_HOME>/omabridge` when
`services.json` says `backend: embedded` and its control socket exists. The host
writes a private `host.json` containing its executable path; desktop routes and
management use that executable, passing the same explicit state directory.
An explicit `OT3_OMABRIDGE_BIN` still takes precedence. Without embedded state,
original T3 / standalone binary and XDG discovery is unchanged.

The host facade implements `manage`, `peers`, `forward` and `t3-pair`. Management
uses `Manager::embedded`, with no systemd backend. Forward/pair use only the live
control socket, never a second endpoint. Embedded management exposes the existing
capability restrictions (relay/gateway changes remain unsupported).
`tools/desktop-bridge-e2e` proves status, read, identity, empty peers,
publish/unpublish, invitation creation, desktop pairing/approval and peer listing using the real desktop executable.
`tests/bridge.rs` separately proves network pairing and forwarding with real protocol
clients. The facade also passes T3 pairing and QUIC descriptor-forward checks. The persistent embedded forwarding lifecycle is described below; standalone sharing retains its unit backend.
No systemd units were edited or installed by this change.

## Verification and remaining limits

`tests/bridge.rs` runs two isolated in-process Nodes. A real network pairing establishes
bridge policy, the guest's control socket issues T3 pairing and starts a real forward,
and the pinned real protocol client exchanges that pairing, verifies it cannot be reused,
and runs `server.getConfig` over authenticated WebSocket. The host has only a Local target,
so no host-side TCP fallback is possible. Socket permissions are asserted.

`tools/measure-bridge-rss <release-o3-server> <debug-or-release-bridge_probe>` copies the
server executable to a fresh disk-backed directory, creates isolated profiles, and samples
VmRSS, VmHWM and RssAnon idle and with one authenticated bridged client. The client runs
in a separate process and is excluded from server RSS. No provider or seeded messages
are included in this bridge sample; the prior seeded baseline is preserved in the JSON.

Remaining limitations: fine-grained subsystem Cargo pruning is deferred (`embed` disables
runtime tasks, but gateway/relay/MCP dependencies still compile). Peer sync is enabled with standalone rollback explicitly disabled. Relay
rollback/reconfiguration remains unsupported in the embedded manager; legacy
`rollback_due` remains standalone-only. Embedded restart retains the QUIC endpoint;
it is not a full Node/network rebind. Public relay/WAN RSS, long-lived reconnects,
gateway route verification and desktop UI integration require deployment-level testing.
Catalogs are populated by `publish_auto` only when verified HTTPS routes are available.
The application still has the core/provider limits documented in `IMPLEMENTATION.md`.

Pre-integration branch sample (2026-10-31, Btrfs): idle VmRSS/VmHWM 27,380 KiB,
RssAnon 17,192 KiB; one bridged client VmRSS/VmHWM 34,200 KiB,
RssAnon 23,348 KiB. Both samples used four threads and zero RssShmem.
The stripped release binaries are 10,643,336 bytes with bridge and 2,319,360 bytes
without it. SHA-256 values and full method are in `rss-measurement.json`.
Builds, worktrees and test state now use `/var/tmp/o3-build/embed` on disk,
following the owner rule. Earlier tmpfs attempts were superseded.
The measured server executable and all measurement state were always on Btrfs.

Branch-local validation before integration: o3-server had 23 passing tests with bridge and 22 without it; format,
`clippy --all-targets -- -D warnings`, and compatibility generation pass.

All development commands must use paths under `/home` (see [DEV_PATHS.md](DEV_PATHS.md)), for example:

```sh
export CARGO_TARGET_DIR=~/.cache/o3-build/main
export TMPDIR=~/.cache/o3-build/tmp
nice cargo test -j 4 --locked
```

The implementation worktrees are `/var/tmp/o3-build/embed/o3-server` and
`/var/tmp/o3-build/embed/omabridge`. Do not use `/tmp` for builds or test state.

Use the short test-state path above: long profile paths can exceed Linux's 108-byte Unix socket address limit. For long production profiles, provide a shorter
explicit `--bridge-state-dir` within the authorized disk directory.

OmaBridge validation on the final disk-backed paths: 165 tests passed across the
full run and the remaining state/T3-pair/units suites. The 2,200-cycle retention
stress test timed out with `connection setup deadline reached` in the full run.
An isolated retry was observed blocked by Btrfs `wait_log_commit` (global I/O
pressure 40–60%) and also ended with the setup timeout after 343.49 seconds. No all-green claim is made for that suite. Re-run after
host I/O pressure clears:

```sh
CARGO_TARGET_DIR=~/.cache/o3-build/main \
TMPDIR=~/.cache/o3-build/tmp \
nice cargo test -j 4 --manifest-path ~/Work/omabridge-embed/Cargo.toml --locked
```

The integrated main workload and binary hash supersede these branch-local RSS
figures; see rss-measurement.json and IMPLEMENTATION.md for current results.

The registered local `feat/embed` worktree is now `~/Work/omabridge-embed`, preserving commit 44cc7fa. Cargo no longer depends on the historical `/var/tmp` checkout. The cross-filesystem relocation used a copy plus `git worktree repair`; the old copy was byte-compared (excluding `.git`) and removed after repairing the registration. No push.

## Host networking parity work (2026-11-01)

`--bridge-gateway-config /absolute/settings.json` opts into TLS gateway hosting.
The JSON has an explicit `bind` IP, a `certificates` map (DNS name to
`[certificate_chain_pem_path, private_key_pem_path]`), and a `gateways` array.
Each entry has `host`, `port`, `peer` (full bridge identity), and `service: "t3"`.
Use `peer: "self"` for this node. Use `127.0.0.1` and a test CA for development. Embedded gateways never discover
machine certificates. A peer equal to this node's identity enters `serve_io`
through a duplex; another identity uses QUIC with the existing public grant.
Local publication withdrawal closes streams on the one-second policy check.
Disabling the gateway option creates no TLS listener or gateway polling loop.
Gateway supervisor shutdown aborts its listener tasks and their connections.

`--bridge-relay-host` explicitly enables the existing relay-host supervisor;
without it there is no hosting poller. Relay *client* selection remains
`--bridge-relay none|public|HTTPS_URL` and uses iroh's connection lifecycle.
No external public relay or DNS is needed by the loopback tests.

`tests/bridge_gateway.rs` exercises a Rust protocol client over a CA-validating
TLS transport adapter, with descriptor verification, one-use pairing and RPC,
for a local gateway and a separate QUIC gateway targeting the embedded node.
This is a Rust client route, not Android UI verification. Publication persistence
and identity restoration are also checked. Desktop persistent *forward sharing*
still needs migration in the desktop repository: its generated units cannot be
fixed solely by changing the host's publication store.

`O3_BENCH_GATEWAY=1 tools/bench-rss <release-binary>` adds a live loopback TLS
listener with an ephemeral test certificate to the standard workload. The output
records `gateway_hosting` so samples remain distinguishable.

Validation of this revision: OmaBridge default features **166 passed** (including
retention stress); o3-server **146 passed, 5 opt-in ignored** with bridge and
**144 passed, 5 opt-in ignored** without bridge. The gateway client verifies
single-use credential rejection as well as successful pairing. Public/WAN relay
reconnection and the desktop's persistent forward lifecycle remain unverified.

Release `tools/bench-rss` samples for this revision (same binary, sequential,
1,000 messages / 20 threads / 3 subscribers / 1,000 RPC; no inference): gateway
off HWM **36,220 KiB (37.09 MB)**; live gateway on **34,404 KiB (35.23 MB)**.
Both are below 50 MB. Sample variation exceeds the small listener overhead;
these numbers do not establish that enabling a gateway reduces memory.
Full aggregates and binary hash: `rss-bridge-hosting.json`.

`tools/bench-rss --profile <private-copy> --bridge-public` now measures a public
relay-configured embedded host, a separate authenticated Rust QUIC client,
largest-thread/shell subscriptions and a short native Claude turn. It does not
start a gateway, force relay-only transport, or verify Android/WAN behavior.
See RSS.md for the allocator ablation and process memory limits.

## Persistent embedded forwards

The host facade accepts `forwards` with a JSON request on stdin:
`{"action":"save","peer":"<paired identity>","service":"t3","listen":"127.0.0.1:47300"}`,
`{"action":"remove","peer":"<paired identity>"}`, or `{"action":"read"}`.
Desired listeners are private atomic `forwards.json` in the explicit bridge state
directory. Writes are serialized with an advisory lock. Only fixed loopback ports
and `t3` are accepted; the registry is capped at 64 entries. A host-owned reconciler
restores listeners after restart, closes removed listeners, retries failures once
per pass, and writes `forwards-status.json` (`listening` or `retrying`). Listening
reports the local socket, not remote reachability or authentication. Each control
setup is bounded to two seconds. Node shutdown drops all control leases and tasks.

The desktop embedded sharing path uses this facade instead of generating units.
Embedded detection survives host downtime; it must never fall back to systemd just
because the socket is temporarily absent. Standalone unit behavior is retained.
The real protocol client regression in `tests/bridge.rs` verifies restoration and
listener removal in isolated profiles. No installed units are migrated automatically.

OmaBridge's loopback relay regression uses an in-process iroh-relay dev server,
relay-only destination addresses and a retained Client across relay/Node restarts.
HTTP is allowed programmatically only for a numeric loopback relay; persisted and
CLI relay parsing still requires HTTPS. The normal regression runs three cycles;
`relay_and_node_restart_stress` opts into 200. It checks live Tokio tasks and RSS
against a warmed baseline (eight tasks / 16 MiB tolerance); these tolerances bound
retention rather than establish zero allocation growth. No WAN claim follows.

Relay validation: the full OmaBridge suite passed, followed by the forced-relay
three-cycle and 200-cycle regressions (49.99 s together). The stress ended at
28 live tasks / 42,312 KiB RSS, versus roughly 27 tasks / 41,504 KiB warmed RSS.
This is transport coverage using the real bridge Client, not a protocol-client
WebSocket reconnection test. The separate o3 regression uses the real protocol
client for persistent-forward restoration. Strict OmaBridge Clippy still reports
existing gateway `collapsible_if`, Node constructor `too_many_arguments`, and
pairing `items_after_test_module` warnings; no clean Clippy claim is made.

Release validation for the persistent-forward host: 147 o3 tests passed with
bridge, 145 without bridge; strict host Clippy passed. `tools/bench-rss` HWM was
16,176 KiB (16.56 MB) with gateway off and 15,792 KiB (16.17 MB) with gateway on,
both below 50 MB. See `rss-forwarding.json` for the common binary hash and workload.
The desktop full suite is blocked by its `approvals` test expecting a different
initial HTTP request; desktop changes remain uncommitted pending that suite.
