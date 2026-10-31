# Integrated implementation

Main combines the core, streaming snapshots/import, Claude and Codex adapters,
attachments/assets/projects/files/VCS/review/search, terminals/operations/queues,
and embedded OmaBridge. docs/compat-status.json is the RPC/command registry;
tools/gen-compat renders COMPATIBILITY.md. Done rows require real-client tests.

The provider contract accepts session input and bounded event/control channels.
A registry routes each thread by providerInstanceId: Claude owns a process per
session, Codex multiplexes one app-server process. Both appear in getConfig with
availability/status; refreshProviders probes the registered drivers. Native frame
reducers feed the same SQLite transcript; events.rs uses common projection
operations. Command events update bounded output tails for real adapters as well
as scripted tests. Claude delivers Bash output when the native tool result arrives;
Codex also streams outputDelta notifications.

There is one queue implementation in thread_commands: ordering, editing, holding,
resuming, cancellation and automatic advance after provider teardown. Direct
steer_active and queued-message.promote-to-steer use the same provider control.
Promotion reserves control capacity before commit and sends after the receipt commits.

Provider turns capture Git trees through a private index, including untracked
nonignored files and deletions. Anonymous commits are retained under
refs/o3/checkpoints; no branch or user index changes. SQLite stores baseline/end
refs and scopes used by getTurnDiff/getFullThreadDiff. checkpoint.rollback restores
the workspace from a ready ref and refuses active executions. Non-Git workspaces
cannot provide checkpoints. Rollback restores files; native provider conversation
rewind is not implemented.

## Remaining gaps

- Durable provider/control replay and active-session reconnection after restart.
  Recovery marks orphan executions failed and requests cancelled; it does not
  continue inference. A crash between a committed receipt and control delivery
  still needs an outbox. Embedded hosts must recover before first transport.
- Provider-native attachments/additional context, full permission policy/setup
  flows, structured Claude questions, full attempt identity and background/subagent
  settlement. Codex authenticated inference/approvals and persisted-rollout resume
  have deterministic-peer coverage, not a live authenticated validation.
  Per-instance custom binaries/environment/model catalogs remain incomplete.
- Checkpoint filesystem restoration and SQLite receipt are separate durable
  effects; crash-safe restoration replay, provider conversation rewind, checkpoint
  retention/garbage collection and overlapping-thread workspace isolation remain.
  Git capture errors leave a turn without a checkpoint; they never fabricate refs.
- Launch create/initial-message receipts are separate, with incomplete failure
  cleanup and reused-thread metadata. Inherited/imported histories do not rebuild
  the complete upstream event log/provider bindings; legacy v1 import is absent.
- Settings schema normalization/annotations/migrations and advanced history
  filtering, throughEntryId, older-page widening and oversized-body transport
  truncation need upstream conformance. Config subscriptions replace snapshots.
- Authentication uses bearer sessions, not cookies/DPoP; existing T3 session rows
  are not imported. OAuth validation/scope negotiation and detailed error/client
  metadata unions remain partial.
- Terminal output is bounded plain text without a full VT screen emulator.
  Scheduled operations, usage/telemetry, PR watches, subagent control, VCS/editor
  variants and native fork semantics have the limits recorded per registry row.
- OmaBridge fine-grained Cargo pruning, host-aware supervisor sync/rollback/rebind
  and production desktop wiring remain (BRIDGE.md). OmaBridge uses the local
  feat/embed checkout at 44cc7fa. Bridge and conformance transport use the same pinned
  client revision. Snapshot tests carry the reference decoder extracted from
  lib-t3-rust 1500d06, whose newer API lacks the bridge catalog module.
  Pin published revisions before distribution.
- No upstream T3 conformance run, desktop/Android end-to-end run, systemd install
  or production deployment. No real T3 profile or running service was accessed.

## Verification and memory

Build and test with TMPDIR=/var/tmp/o3-build/tmp and
CARGO_TARGET_DIR=/var/tmp/o3-build/main. Use nice cargo test -j 6 --tests,
nice cargo clippy -j 6 --all-targets -- -D warnings, nice cargo fmt --check,
and nice cargo build -j 6 --release --features bridge. Regenerate tools/gen-compat.
Run nice tools/bench-rss /var/tmp/o3-build/main/release/o3-server afterward.

Import uses SQLite read-only backup into TMPDIR, never the real T3 database for
tests. Tests use synthetic profiles and repositories. RSS evidence is recorded
in rss-measurement.json from the integrated release executable with bridge enabled
and both drivers registered/probed. The workload has 20 threads, 1,000 deferred
messages, three bounded snapshot subscribers and 1,000 settings calls. Provider
inference/subprocess memory and sustained slow-client concurrency are excluded;
this is a measured workload, not a universal memory bound.
