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

## Build root

Tools under `tools/` keep their state under `$O3_BUILD_ROOT`, default
`~/.cache/o3-build` (`main/` for the release build, `tmp/` for temporary state,
`android/`, `qml/`, `e2e/` for the end-to-end harnesses). Rust tests write only
below `$TMPDIR`.

## Test leftovers

Test harnesses create profiles such as `o3-it-*`, `o3-mcp-*`,
`o3-conformance-*` and `o3-shell-*` in `$TMPDIR` and do not remove them: tests
never delete trees. Run tests so the runner owns the cleanup:

- `tools/test-rust-sandbox` runs the tests in Podman with a disposable `/tmp`.
- `tools/cargo-test <cargo test args>` runs on the host with a private
  `TMPDIR` under `$O3_BUILD_ROOT/tmp` and removes it when cargo exits.

A bare `cargo test` with `TMPDIR` pointing at a shared directory leaves these
profiles behind.

Historical reports that mention `/var/tmp` record where a measurement ran; they
are not instructions.
