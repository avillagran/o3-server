# Immutable private runtime artifacts

The compilation lock alone does not freeze a shared Cargo target directory after
compilation ends. Another worktree can replace a test binary, runtime library, or
`CARGO_BIN_EXE` server while a long collection is still running.

`tools/test-scheduled-modes` now copies Cargo's executable outputs, top-level
executables and `deps/*.so` runtime libraries before releasing the compilation
lock. Each run retains its own artifact directory alongside the resolved Cargo
target, avoiding large copies on the root filesystem when the target is on a
separate build volume. The result manifest lists every copied artifact with its
SHA-256 and byte length. The private container mounts these copies both as
`/test-bin` and at the original embedded debug path. It never mounts the mutable
shared debug directory as runtime input.

The copier rejects symbolic-link artifacts and paths outside the debug root.
It copies individual regular files and retains its output; it does not recursively
remove anything. Private regression tests prove that later writes to the original
executables and shared library do not change the snapshot, and that links and
outside artifacts are refused.

This freezes binary inputs, not source checkout contents. Full acceptance still
requires a clean immutable source worktree, a known commit, the filesystem safety
preflight, and a completed runtime manifest. Historical shared-target results must
not be presented as new immutable full-matrix acceptance. A reported failed test
also needs individual provenance review before attributing it to a source commit.
