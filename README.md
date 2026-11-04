# o3-server

A lean Rust replacement for the headless T3 Code server. One binary speaks the T3 wire protocol
(RPC over WebSocket, HTTP routes, `t3` CLI and `T3CODE_HOME` profile) so lib-t3-rust, O3 desktop
and O3 Android can talk to it like T3, with OmaBridge built in. Claude runs through the CLI's
`stream-json` mode and Codex through `app-server`; no Node.

## Status

Not a finished replacement. M2 runs a reversible service trial using an isolated profile;
daily-use acceptance is tracked separately from protocol coverage.

- Compatibility is tracked per RPC and command in [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md),
  with evidence and limits in [docs/CONFORMANCE.md](docs/CONFORMANCE.md).
  The goal is full T3 Code parity; previously skipped surfaces remain backlog.
- Development integrates on `dev`; `main` receives explicitly validated promotions.
  [Nightly maintenance](docs/UPSTREAM.md) documents the automated source monitor,
  port ledger and stability gates. Detection never promotes a nightly automatically.
- Memory depends on workload and platform: see the controlled M2/NODE measurements below
  and the separate real-history workloads in [docs/RSS.md](docs/RSS.md).
- Verified against the real lib-t3-rust client, real Claude and Codex turns, the O3 desktop backend
  and QML UI, and O3 Android on an emulator: pairing, new threads, chat, approval, reconnect, an image
  attachment and terminal commands ([docs/E2E.md](docs/E2E.md)). Not verified: daily use and Android through an
  OmaBridge gateway, which needs the real gateway deployment.

## M2 and NODE comparison

Measured on 2026-11-03. Both release binaries use source `7c9223e` (the same tree promoted
into `main` as `c9c667b`). These are Linux hosts; M2 runs Asahi Linux, not macOS.

| Host | Processor | Architecture | Logical CPUs | RAM available to Linux |
|---|---|---|---:|---:|
| M2 | Apple M2 Pro (8 performance + 4 efficiency cores) | aarch64 | 12 | 15,379 MiB |
| NODE | AMD Ryzen 9 5900X | x86_64 | 24 | 31,979 MiB |

The same isolated synthetic workload seeds 1,000 deferred messages of approximately 1 KB in
one thread, then attaches one authenticated WebSocket client. Its bounded snapshot contains
10 messages. No provider inference, desktop UI, browser or Android process is included.
Bridge support is enabled; no public relay traffic is generated.

| o3-server metric | M2 | NODE |
|---|---:|---:|
| Release binary, decimal MB | 14.44 | 16.10 |
| Startup RSS, decimal MB | 15.01 | 11.79 |
| Seeded RSS, no subscribed client, decimal MB | 17.04 | 17.95 |
| Seeded RSS, one bounded subscription, decimal MB | 16.97 | 17.95 |
| Durable messages seeded | 1,000 | 1,000 |
| Messages in the bounded snapshot | 10 | 10 |

One run per host, sampled from Linux `/proc/PID/smaps_rollup`; these are observations,
not peak-memory limits or CPU speed rankings. Hosts also ran other services. The startup
sample is taken immediately after runtime discovery. See the [raw aggregate evidence](docs/platform-measurements.json)
for timestamps, KiB values and binary hashes. Reproduce with a release build:

```sh
TMPDIR=/var/tmp O3_BENCH_SOURCE_SHA=$(git rev-parse HEAD) tools/measure-rss /path/to/release/o3-server
```

For context, the live T3 serving process was also observed on each host. Those profiles,
versions and activity differ from each other and from the synthetic workload above, so
these figures do **not** establish a controlled T3-to-o3 memory reduction:

| Live T3 observation | M2 | NODE |
|---|---:|---:|
| T3 nightly | 3904 | 3991 |
| Serving-process RSS, decimal MB | 287.20 | 625.53 |

These live samples use `/proc/PID/status` (`VmRSS`) and exclude launchers, provider children
and clients. Full compatibility remains pending: `/mcp` now provides authenticated
readers and thread control with dedicated credentials and automatic Claude/Codex injection
([contract and gaps](docs/MCP.md)); project mutations, worktree readers and calling-thread
worktree handoff with setup/continuation are also available. Thread launch/restart/transfer,
remaining toolkits, OpenCode/ACP and DPoP/browser authentication remain pending. Native Claude/Codex support does not imply those capabilities are available.

## Build and test

```sh
cargo build --release -j 4
cargo test -j 4
```

Keep build output off tmpfs, for example `CARGO_TARGET_DIR=/var/tmp/o3-build/main`.

## Try it beside T3 (pilot)

`tools/pilot` runs o3-server next to your running T3 without touching it: its own port (chosen by
the kernel), its own `T3CODE_HOME` under `~/.local/share/o3-pilot`, and your T3 history imported
from a read-only copy of `~/.t3/userdata/statev2.sqlite`. T3 is never written to or signalled.

```sh
tools/pilot                 # import history, start, print a one-time pairing link
tools/pilot --stop          # stop it (the pilot dir and its history stay)
tools/pilot --resume        # start the same pilot again with a fresh pairing link
```

Add it in O3 desktop under Settings → Connections → Add connection: paste the printed
`http://127.0.0.1:<port>/pair#token=...` link. Then pick that connection for a thread; T3 keeps
serving the others.

`tools/pilot` refuses to start if the pilot dir already exists (remove it yourself for a clean
import). Options: `--t3-home DIR`, `--bin PATH`, `--no-import`, `--ttl 30m`. `O3_PILOT_DIR`
moves the pilot dir.

## Other tools

`tools/gen-compat` regenerates the compatibility table from `docs/compat-status.json`;
`tools/measure-rss`, `tools/bench-rss` and `tools/conformance-diff` produce the RSS and
differential evidence. Architecture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
