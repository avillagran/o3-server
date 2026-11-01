# o3-server

A lean Rust replacement for the headless T3 Code server. One binary speaks the T3 wire protocol
(RPC over WebSocket, HTTP routes, `t3` CLI and `T3CODE_HOME` profile) so lib-t3-rust, O3 desktop
and O3 Android can talk to it like T3, with OmaBridge built in. Claude runs through the CLI's
`stream-json` mode and Codex through `app-server`; no Node.

## Status

Not a finished replacement and not installed anywhere. T3 stays the daily server.

- Compatibility: 78 of 203 RPCs done, 40 partial, 85 out of scope (77 no client uses, 8 answer T3's typed unavailable error).
  P0 is 0 of 5 done (daily chat, shell and thread streams, config, dispatch); P1 is 20 of 31.
  Full table and per-RPC limits: [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md); evidence and
  accepted differences: [docs/CONFORMANCE.md](docs/CONFORMANCE.md).
- Memory: about 31 MB idle and 42 MB peak with a real 374-thread history ([docs/RSS.md](docs/RSS.md)),
  against 579 MB for T3 headless.
- Verified against the real lib-t3-rust client, real Claude and Codex turns, and the O3 desktop
  backend ([docs/E2E.md](docs/E2E.md)). Not verified: the QML desktop UI and Android.

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
