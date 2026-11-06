# Shared command admissions

The forward-going bridge targets the official T3 nightly `0.1.8-nightly.20261106.4262` and its
migration 80 database. It shares accepted `thread.create` and new-run `message.dispatch`
admissions. It does not establish parity for every command receipt or provider control effect.

O3 records the command ID, native command type, thread, acceptance time, original local creation
event, run and user-message IDs in the same SQLite transaction as its own receipt and records.
Native export links the original `thread.created`, `run.created` and user-message event to that
command ID, even if the current projection has advanced. It inserts an accepted native receipt
only after those events and projections exist, with the native thread aggregate and resulting
native event sequence. Launches preserve both the base command and its `:initial-message` alias.
T3's launch replay needs the original command-linked `run.created` event to recover its run ID.

Foreground admission and provider/preparation fences serialize with background export without
holding the O3 database mutex across export. Provider entry first commits a local start claim and
`starting` run projection, then persists a native `provider-turn.start` outbox row with status
`running`, attempt count 1 and lease owner `o3`. Native restart recovery treats that row as a lost
process-bound effect and cancels it. It does not silently repeat a scheduled message whose provider
may already have accepted work before the `Started` callback. Terminal export settles the claim.
An export failure holds admitted preparation or a durable deferred provider start until retry;
it cannot enter the provider adapter first.

These are separate databases: the fence is not a distributed transaction. A crash before native
admission may leave an unmirrored local admission, but the guarded preparation/provider entry has
not been released. Older private receipts without original admission evidence are not upgraded
into invented native successes. A profile without a native database retains ordinary unshared
behavior. Migration 73 remains usable for existing supported history, but nonempty canonical
webhook pending rows require migration 80 before a sharing fence can succeed.

Native accepted receipts import without creating replacement messages. Recovery can reuse a
proven unstarted root preparation or an unstarted queued run. It checks native projections,
provider turns and start outbox delivery state. A native worktree/setup operation that may already
have had effects, or an already claimed/unknown native start, remains explicitly held with
`shared_admission_recovery_required`; this conservative hold is a remaining recovery parity gap.
Rejected receipts, controls/steering, reused-thread launch metadata and the complete native effect
outbox are also outside this bridge. No full generic interchangeability claim follows from it.

## Acceptance evidence

`tests/shared_command_receipts.rs` has ten ordinary container regressions covering exact receipt
fields and linked events, immutable creation snapshots, launch alias replay, partial admissions,
thread conflicts and transactional rollback, native root/queued recovery, held ambiguity,
receipt and effect-claim write failure, and migration 73 admission refusal without local loss.
The ordinary suite passes with minimal and default Cargo features.
The command-sharing unit matrix additionally verifies 18 native pending-worker combinations:
both command ID forms, all three held recovery statuses, and preparing/starting/queued runs.
Held rows retain their replay claims, while resolution, execution progress and terminal delivery
cleanup become eligible without a busy retry loop.

Three ignored acceptances run the actual official nightly without inference:

1. An O3 launch's base and initial-message receipts replay through official
   `orchestration.launchThread` with the same run and no additional provider turn.
2. A real official `defer_start` message imports into O3 and recovers the same root run once;
   replay creates neither another message nor another provider callback.
3. A synthetic provider pauses before emitting `Started`. Its entry verifies the native
   `starting` projection and claimed outbox row. A crash snapshot handed to the official runtime
   is cancelled by native recovery; launch replay retains the original run and no provider turn.

All tests require rootless containers, read-only source/runtime mounts and private temporary
profiles. Run the ordinary cases with `tools/test-scheduled-modes --integration
shared_command_receipts` (also with `--default-features`) and the official acceptances with
`tools/test-headless-reference <nightly-directory> --test shared_command_receipts`.
