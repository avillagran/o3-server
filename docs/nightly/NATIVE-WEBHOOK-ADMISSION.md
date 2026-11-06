# Native webhook admission and execution handover

New HTTP deliveries use the native scheduled-task ledger. Previously accepted
O3 deliveries continue through their legacy queue; they are not converted into
invented native requests. This candidate does not yet certify the complete
scheduled-task service or global nightly parity.

The reference is official T3 nightly 4262 (`c55db3e9`), using its exact contracts
and Effect 4.1.2. The covered scheduled-task sources remain unchanged in 4275
(`6577c6b6`); other 4275 changes retain their own audit requirements.

## Admission and retained work

GET, POST, PUT and PATCH return the native 202 response containing `deliveryId`
and `x-t3-hook-outcome`. HEAD returns 405 without starting work. Bodies are limited
to 1 MiB. Unknown or rotated endpoint tokens return 404. Unsigned token-authenticated
hooks and optional HMAC verification use the native filter/template helpers.

Admission writes redacted delivery receipts, seven-day replay claims and native
pending rows transactionally. Coalescing retains the first deadline and task
snapshot, updates the latest request and keeps all member delivery IDs, including
members omitted by the bounded log budget. Per-task run/rejection limits are
60/minute, queued work is capped at 20 and filtered/coalesced log limits are
10/minute. Failed dispatch releases replay claims according to the native policy.
Receipt bodies are bounded to 64 KiB without a trailing partial UTF-8 character;
rendered fields use UTF-16 limits. Receipt insertion and retention pruning roll
back together on failure.

Immediate requests also retain a durable pending row in `running` state. This is
an O3 storage extension: the reference holds immediate execution in its fiber and
in-memory queue. It preserves the observable `pendingWindow: false` behavior and
allows O3 to recover admitted work after a crash.

The worker uses the canonical scheduled command/message IDs, validates task
identity and pause/trigger/policy fences, retains ambiguous imported admissions,
and records each terminal outcome once. A database failure preserves pending
work instead of deleting it. Current task prompts replace the saved prompt only
at the native pre-dispatch boundary; already-running attempts keep their snapshot.

## Shared effect boundary

A shared migration-80 profile receives the original creation events, their
`command_id` linkage, projections and accepted native command receipts before a
successful admission response or preparation/provider effect. Foreground and
background exports share a mutex so an older snapshot cannot overwrite a newer
admission. Failed native writes retain local work and an explicit recovery state.
A schema-73 profile with pending native webhook work requires an upgrade to 80.

Creation snapshots are immutable even when later run updates exist. Launch replay
uses both the base `thread.create` receipt and its `:initial-message` admission.
Only imported preparation proven not to have started is reconstructed; ambiguous
worktree/setup/claimed starts remain held for explicit recovery.

Before calling a provider, O3 commits `starting` plus a process-bound native
`provider-turn.start` claim. Native restart recovery cancels that claimed attempt
rather than blindly sending it again. A receipt proves admission; it does not
prove an external effect completed. The journal does not claim exactly-once
external execution.

## Evidence

- `tools/generate-webhook-admission-fixtures.mjs` executes the pinned verification
  module and template validator with Effect 4.1.2. Its 360 signature and 21 template
  cases include empty HMAC keys, prefix/encoding/whitespace boundaries and native
  decoder behavior. The empty-key comparison exposed a corrected O3 mismatch.
- `tests/native_webhooks.rs` uses real RPC and HTTP to check method/outcome IDs,
  trusted mentions, redaction and completion-write failure. A retry retains one
  user message and one counted run.
- `tests/fresh_context_live.rs` compares ten real HTTP responses and normalized
  receipt details with the official runtime, including oversized prompts,
  acceptance/coalescing/duplicates, filtering, HEAD, disabled tasks and bad tokens.
  The private scenario starts no inference and also retains the existing shared
  task/read/token-rotation checks.
- `tests/shared_command_receipts.rs` checks mirror-write failures before
  preparation/provider callbacks, admitted replay, safe imported preparation,
  ambiguous holds and the schema-73 boundary. Three opt-in tests start the actual
  official runtime: O3 receipts replay in T3; native deferred admission executes
  once in O3; a pre-Started crash handover cancels the claimed attempt without a
  second run or provider turn.

All tests and oracle generation execute in rootless Podman with read-only inputs,
private temporary state and no external network. The official-runtime acceptance
uses no provider credentials or inference.

## Legacy profile readiness

`o3-server shared-profile-readiness [--base-dir PROFILE]` inspects
`PROFILE/userdata/o3.sqlite` before server startup, ownership checks, migrations
or policy loading. It opens the existing database read-only and counts legacy
`hook_queue` and `hook_replay` rows in one SQLite snapshot. It never drains a
queue, expires a claim, reads request payloads or creates a missing database.
Missing databases with orphaned WAL/SHM files, links, corrupt databases and
unexpected schema objects fail the check.

The version-1 JSON report contains `legacyWebhookReady`,
`pendingLegacyDeliveries` and `retainedLegacyReplayClaims`. Exit status 0 means
the diagnostic succeeded; callers must inspect the report, since retained data
also returns 0. Invalid arguments and unreadable state return 2. A ready report
certifies this legacy webhook boundary only, not the entire shared profile.

Every retained replay row blocks handover, even when its stored expiry is in
the past: the legacy admission path checked key presence without consulting
that expiry. Draining the queue alone cannot preserve duplicate suppression
when switching to native T3.

The desktop candidate checks this report before stopping O3 to switch to T3
on a shared profile, then checks again after O3 stops and before T3 starts.
Start, stop, restart and switch operations share the local control lock.
Unverifiable or inconsistent reports refuse the switch. Existing data remains
intact and O3 remains selectable for reconciliation. Starting T3 outside the
desktop control path bypasses this guard and is not certified by it.

Seven container integration checks cover absent files, retained deliveries and
claims, committed WAL snapshots, invalid schemas, links and the real diagnostic
CLI under foreign profile ownership. Desktop integration checks cover refusal
before any stop/start effect, malformed reports and work admitted during graceful
shutdown. A lossless legacy migration remains open because those rows do not
retain the original native request or task snapshot.

## Remaining work

Relay proof verification, relay-origin URLs, secret providers and global managed
policy still need implementation and acceptance. Imported ambiguous preparation
needs its complete ownership/reconciliation path. Legacy accepted O3 rows retain
only a rendered prompt and expiry. The desktop readiness guard prevents switching
with retained deliveries or replay claims; lossless migration and launches outside
that control path still need a compatibility boundary. Complete due-window,
contention, project/thread movement and platform lifecycle comparisons remain in
the full parity goal.
