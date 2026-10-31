# Embedded OmaBridge

The default `bridge` Cargo feature runs OmaBridge on o3-server's two-worker Tokio runtime.
`--no-default-features` removes it. No bridge executable or service is started by the server.

The local development dependency is patched to `/var/tmp/o3-build/embed/omabridge`, branch
`feat/embed`, commit `44cc7fa7575c1256f4007be4823e3f455429175a`. Before release, replace this patch with a published, pinned git revision.
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
Embedded `NodeConfig` enables the control socket; peer sync, gateway hosting and relay
hosting loops are disabled. Explicit pairing/hello/control operations remain available.
The control socket bind must succeed and its mode is 0600.

## Desktop compatibility and required changes

The wire protocol of `peers`, `forward`, `connect`, `t3-pair` and remote `manage` is retained.
The existing client-side `omabridge` CLI may still be used; the server does not need it.

For a desktop using the embedded host, make these changes in
`/home/node/Work/omarchy-t3-code` (not edited here):

1. `src/omabridge_route.rs`: configure `stateDir` to the same explicit directory used
   by o3-server. The existing `wait_for_server`/`forward` protocol needs no changes.
2. `src/omabridge_share.rs`: replace `After=... omabridge-serve.service` in generated
   forward units with the actual o3-server unit, or remove the bridge-unit ordering.
   Keep the `control.sock` readiness check and the client-side forward command.
3. `src/omabridge_device.rs`: pass `--state-dir` when invoking `manage`; it currently
   relies on CLI defaults. Use a CLI built from `feat/embed` for local publish/unpublish
   against embedded state; older CLIs assume systemd and report already-running.
4. Gate gateway/relay-host UI on returned capabilities and disable relay mode changes
   when `published.backend == "embedded"`. Embedded mode rejects
   `relay-set`, `relay-host`, `gateway-set`, `gateway-domain-set` with
   `omabridge_embedded_unsupported`; configure relay at server startup instead.
5. Keep the T3 executable/base-dir mapping pointed at o3-server for session list/revoke.
   No label-format change is needed.

Eliminating the client-side CLI as well requires replacing the desktop's CLI subprocesses
with Store reads and JSON requests to the private control socket. This is separate from
eliminating the host's bridge service/executable, which this implementation does.

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
runtime tasks, but gateway/relay/MCP dependencies still compile). Peer sync and relay
rollback/reconfiguration need a host-aware supervisor before enabling those loops;
legacy `rollback_due` remains standalone-only. Embedded restart retains the QUIC endpoint;
it is not a full Node/network rebind. Public relay/WAN RSS, long-lived reconnects,
gateway route verification and desktop UI integration require deployment-level testing.
Catalogs are populated by `publish_auto` only when verified HTTPS routes are available.
The application still has the core/provider limits documented in `IMPLEMENTATION.md`.

Latest release sample (2026-10-31, Btrfs): idle VmRSS/VmHWM 27,380 KiB,
RssAnon 17,192 KiB; one bridged client VmRSS/VmHWM 34,200 KiB,
RssAnon 23,348 KiB. Both samples used four threads and zero RssShmem.
The stripped release binaries are 10,643,336 bytes with bridge and 2,319,360 bytes
without it. SHA-256 values and full method are in `rss-measurement.json`.
Builds, worktrees and test state now use `/var/tmp/o3-build/embed` on disk,
following the owner rule. Earlier tmpfs attempts were superseded.
The measured server executable and all measurement state were always on Btrfs.

Validation: o3-server has 23 passing tests with bridge and 22 without it; format,
`clippy --all-targets -- -D warnings`, and compatibility generation pass.

All development commands must use disk-backed paths, for example:

```sh
export CARGO_TARGET_DIR=/var/tmp/o3-build/embed/target
export TMPDIR=/var/tmp/o3-build/embed/t
cargo test --locked
```

The implementation worktrees are `/var/tmp/o3-build/embed/o3-server` and
`/var/tmp/o3-build/embed/omabridge`. Do not use `/tmp` for builds or test state.

Use the short test-state path above: long profile paths can exceed Linux's 108-byte Unix socket address limit. For long production profiles, provide a shorter
explicit `--bridge-state-dir` within the authorized disk directory.
