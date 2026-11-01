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

- Active inference recovery and native control replay after restart. Idle native IDs survive and are supplied to both adapters on the next message.
  Recovery marks orphan executions failed and requests cancelled; it does not
  continue inference. Receipt-committed controls now have a durable handoff journal; native delivery ambiguity and safe replay remain (RECOVERY.md). Embedded hosts must recover before first transport.
- Full permission policy/setup flows, full attempt identity and background/subagent
  settlement. Native uploads/context and structured Claude questions are implemented. Codex authenticated inference/approvals and persisted-rollout resume
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
  and full desktop sharing-unit wiring remain (BRIDGE.md). OmaBridge uses the local
  feat/embed checkout at 44cc7fa. Bridge and conformance transport use the same pinned
  client revision. Snapshot tests carry the reference decoder extracted from
  lib-t3-rust 1500d06, whose newer API lacks the bridge catalog module.
  Pin published revisions before distribution.
- Partial isolated upstream differential evidence is now recorded in CONFORMANCE.md.
  Desktop/Android end-to-end, full native inference parity and production deployment
  remain unverified. The real user T3 profile and running service were not accessed.

## Verification and memory

Build and test with TMPDIR=/var/tmp/o3-build/tmp and
CARGO_TARGET_DIR=/var/tmp/o3-build/main. Use nice cargo test -j 4 --tests,
nice cargo clippy -j 4 --all-targets -- -D warnings, nice cargo fmt --check,
and nice cargo build -j 4 --release --features bridge. Regenerate tools/gen-compat.
Run nice tools/bench-rss /var/tmp/o3-build/main/release/o3-server afterward.

Import uses SQLite read-only backup into TMPDIR, never the real T3 database for
tests. Tests use synthetic profiles and repositories. RSS evidence is recorded
in rss-measurement.json from the integrated release executable with bridge enabled
and both drivers registered/probed. The workload has 20 threads, 1,000 deferred
messages, three bounded snapshot subscribers and 1,000 settings calls. Provider
inference/subprocess memory and sustained slow-client concurrency are excluded;
this is a measured workload, not a universal memory bound.

Integration verification (2026-10-31): 91 tests pass with bridge enabled,
90 pass without default features; each run leaves two live-provider tests
explicitly ignored. Strict all-target clippy, cargo fmt --check and git diff
--check pass. Both runs use serial test execution to avoid shared-disk deadline
failures. The registry contains 32 done, 25 partial, 69 todo and 77 skipped RPCs.

Final release sample on Btrfs: VmRSS 40,764 KiB, VmHWM 41,252 KiB,
RssAnon 29,420 KiB after churn (under both 50 MB and 50 MiB). The bridge
published its isolated endpoint before load. Claude was available with unknown
authentication; the Codex probe reported error in the empty isolated profile.
Neither adapter performed inference in this memory workload.


Operational checkpoint: provider discovery is lazy and uses resolved native executables,
not HOME-sensitive mise wrappers. Codex's app-server and cached Claude sessions expire
after 60 seconds idle. Claude's advertised default instance is claudeAgent; old claude
and claude-code selections remain aliases. See CONFORMANCE.md for scope and evidence.

Current local validation: 101 tests pass, 3 live/reference tests ignored; strict clippy passes. Desktop embedded management has isolated real-client proof (E2E.md). See RECOVERY.md for handoff guarantees and remaining gaps.

Final no-default-features validation: 100 tests pass, 3 ignored. Desktop OmaBridge-filtered tests: 20 pass. The complete desktop suite was not run.

## Daily-chat implementation checkpoint (2026-11-01)

Launch now stages thread creation, the initial user message/context/run, events and
receipt in one SQLite transaction, then starts the adapter. Fault injection through
the real client proves rollback and idempotent retry. Worktree preparation remains
outside SQLite and can leave a prepared worktree after a later database failure.

Uploaded attachments resolve only through the profile attachment store; client paths
are ignored, and symlink entries are refused. Claude receives supported images as
bounded base64 blocks (5 MiB/image, 8 MiB/frame), Codex as localImage inputs. Other
files are supplied as quoted local file references for native tools. Composer context
is serialized into a user text block; provider-specific skill/mention expansion is
not yet implemented. Tests cover actual upload -> real Connection -> native peer.

Claude AskUserQuestion projects structured questions recognized by the shared client
helper and converts string/array answers to SDK answers keyed by question text.
The pinned reference manifest supplies model IDs and capability descriptors; effort,
fastMode, thinking and context-window settings reach CLI argv. Option changes cause
an idle resume instead of silently retaining the previous process settings. Native
slash-command/skill lists become available after a real session init, not at startup.

This checkpoint does not close all five P0 rows or the remaining P1 partials. In
particular persistent permission rules, active recovery, full plan workflow, usage
summary/quota, generated titles and safe automatic settlement remain open.

## Daily behavior follow-up

Exact-operation persistent consent, native Claude/Codex plan acceptance and
source-plan completion, observed Claude quotas, explicit deterministic title
regeneration and conservative age settlement are implemented. They do not imply
wildcard permission policy, semantic title inference, proactive quota discovery or
merge settlement. Recovery finalizes orphan turn items/nodes/messages; inference
is failed honestly rather than resumed. Linked-worktree checkpoint tests verify
file restoration without touching the root checkout or either index.

## Client fields checkpoint (2026-11-01)

The latest checkpoint supersedes older field-gap descriptions above: attempts/nodes
exist while preparing, Claude binds attempts to provider turns, Codex advertises
native reasoning choices, observed context usage is projected, and stream events
carry run/node/instance metadata. Filesystem startup skills/commands, preference
mutations, project Git identity and replacement snapshots, force-delete shell
cascade, supplemental search shells, GitHub PR status, remaining signed asset tags
and terminal instance environment have real Connection tests. CONFORMANCE.md lists
precisely what remains partial; this is not full daily-client interchangeability.
