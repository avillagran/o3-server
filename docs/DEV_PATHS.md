# Development paths

Build output, temporary files and test state belong under `/home`, normally in
`~/.cache/<project>/`:

```sh
export CARGO_TARGET_DIR=~/.cache/o3-build/main
export TMPDIR=~/.cache/o3-build/tmp
```

Do not use:

- `/tmp`: a RAM-backed tmpfs. Target directories and test profiles fill it and
  stall processes that wait for memory.
- `/var/tmp`: it lives on the root filesystem, which is small and on a slower
  disk. On 2026-11-11 about 11,000 leftover test directories there exhausted the
  root Btrfs metadata. No file could be created on `/`, so SQLite reported
  `unable to open database file` and `cc` could not create link temporaries.

The production service sets the same rule for agents it launches:
`TMPDIR=/home/node/.cache/o3-tmp` in `o3-server.service`.

## Known leftovers

- `tests/support/mod.rs` creates an `o3-it-*` profile in `$TMPDIR` for every
  test server and does not remove it. Other harnesses leave `o3-mcp-*`,
  `o3-conformance-*` and `o3-shell-*` directories. Point `TMPDIR` at a per-run
  directory and remove that directory from the runner after the run, never from
  inside a test.
- Several tools and `tests/differential.rs` still hard-code `/var/tmp/o3-build`
  (`tools/bench-rss`, `tools/live-chat-e2e`, `tools/rss_profile.py`,
  `tools/conformance-diff`, `tools/desktop-bridge-e2e`, among others). Moving
  them to a configurable root under `/home` is pending.

Historical reports that mention `/var/tmp` record where a measurement ran; they
are not instructions.
