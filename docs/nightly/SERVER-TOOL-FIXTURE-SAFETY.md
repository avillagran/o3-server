# Server tool fixture retention

Server runtime probes require the same private Podman or verified GitHub Docker
admission as Rust fixtures. The guard runs before child processes or credential
copying. It is not a container launcher: callers still supply read-only source,
private HOME/tmp and the required network policy. Live credential/inference tools
remain explicit opt-in operations and are not invoked by the default matrix or
by the filesystem audit.

`runtime_safety.retained_directory` creates an exclusive mode-0700 workspace with
`mkdtemp` and never recursively cleans it on normal exit or failure. CI's offline
nightly-watch regression uses this adapter, as do the RSS, shell-scale and legacy
live/bridge probes. Their assertions are unchanged. Fixture executables are copied
into the owned workspace instead of linking to external executable paths. The
remaining RSS profile alias targets only its own newly created profile copy.

Pure admission/AST checks and private retention tests verify host refusal before
fixture creation, CI marker requirements and preserved contents after exceptions.
They never run a benchmark, installed provider, credential copier or live turn.
Retained data remains in the disposable container until teardown; do not bind its
private temporary storage to a real user profile.

## Reviewed server audit boundary

The default server Cargo matrix and its CI Python nightly-watch step contain no
remaining explicit recursive fixture cleanup or auto-cleaning directory context.
The four Git worktree deletion consumers use explicitly injected retaining
executors in their affected fixtures. Browser profile clear uses its shared
removal plan with a recording executor. Remaining owned-file unlink and empty
directory removal paths do not recursively traverse trees.

Link fixtures in server Rust tests target their own private workspace; executable
resolution and privilege tests copy binaries or create owned helpers. Native
reference filesystems/processes are mocked where destructive source operations
are recorded. The pinned OmaBridge dependency's `tempfile::tempdir` occurrences
are confined to that dependency's own `cfg(test)` modules, which the server
package's Cargo invocation does not execute.

This closure recommendation excludes the separately audited desktop/QML harness
and does not run ignored live-provider tests, real SDKs, benchmarks or hardware.
The broad-runtime audit marker remains controlled by the parent audit owner.
