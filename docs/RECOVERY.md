# Native session and control recovery

Idle restart keeps Claude `activeProviderThreadId` and Codex `codexNativeThreadId`
in the thread projection. The next supervisor run supplies them as native session
IDs; adapters use Claude `--resume` and Codex `thread/resume`. Recovery disconnects
old provider-session projections without erasing conversation IDs. A missing Codex
rollout now fails explicitly instead of silently starting a new conversation.
Native CLI state must still exist in the same isolated provider home.

`provider::restart_tests::idle_restart_supplies_durable_native_ids_to_both_adapters`
closes/reopens SQLite and verifies the next adapter input for each driver. The
Codex deterministic peer test now closes the app-server before resuming. Existing
Claude native resume tests cover command construction/live resume when explicitly
enabled; a new authenticated two-process live restart was not run here.

Controls (steer, queued promotion, interrupt, runtime response/dismiss) write a
`controlOutbox` record in the receipt transaction. Only after commit does the
reserved bounded channel receive the control. `handed-off` means channel delivery,
not a native execution acknowledgement. Restart converts pending and handed-off
records to `delivery-unknown`, preserves the command, and emits an ordinary thread
metadata update carrying `lastControlDeliveryError`. It does not blindly resend an
approval into a different native turn. The atomicity/rollback test in `tests/core.rs`
verifies durable evidence across reopening. Receipt retries remain idempotent.

Pending approvals are cancelled with an explicit `resolutionReason` explaining that
the native channel died and a new message is needed to request approval again.
Active runs without a durable supported native session fail with a restart error.
Runs with Claude/Codex native IDs enter `starting` with `recoveryPending` and a
visible `recoveryReason`, then continue automatically after provider registration.
This is a durable handoff journal, not exactly-once native delivery or a replay worker.
The native protocols do not expose an idempotent command acknowledgement across
restarts. Records are retained in SQLite; retention/GC and frontend presentation of
the new error metadata remain future work. Never claim full native chat parity.

Launch checkpoint: the thread, initial message/context/run, event rows and launch
receipt now share one transaction. Native execution starts after commit. A failed
run insert leaves none of those records; retrying the same launch creates one run.
This improves database atomicity, not crash replay of committed native starts or
filesystem rollback of a newly prepared worktree.

Restart now also finalizes orphan provider nodes/attempts/subagents and tool items,
stops assistant message streaming, and emits ordinary projection update events.
Deferred and held/queued work is preserved. A second recovery is idempotent.
The run's normal error/status fields carry the restart failure, which clients
already read; consumers need not understand lastControlDeliveryError to see that
execution stopped. The orphan projections close before continuation starts.

## Active execution continuation

This is native conversation **resume with a new turn**, not reattachment to the
old inference process. Claude uses `--resume`; Codex uses `thread/resume` followed
by `turn/start`. The original run ID stays stable. A dedicated continuation prompt
asks the provider to inspect state, avoid repeating completed work, and ask again
for cancelled approvals/questions. Original attachments/context are not resent.
`recoveryReason` stays on the run in normal `run.updated` events after completion;
`recoveryPending` clears on provider start. Native resume errors fail visibly;
there is no fallback to a new conversation. Missing native IDs fail conservatively.
Tools may have executed before the crash: this does not promise exactly-once side
effects, replay the original user prompt, or recover the old native control channel.

`tests/active_recovery.rs` seeds durable native sessions, starts the actual server
against deterministic Claude/Codex stdio peers, SIGKILLs it during execution, and
starts a second process. Both native IDs are asserted by peers; the same run
completes, recovery metadata survives, and the old request is visibly cancelled.
The peers exit on stdin EOF so killed servers do not leave inference running.

Live verification (2026-11-01, NODE): installed Claude Haiku, copied credentials
and native transcript in an isolated `/var/tmp/o3-build` home. A short seed turn
recorded a pending sleep/reply task. The real server resumed it, was SIGKILLed
after the run became running, and a second process resumed the same native
session/run to `completed`. Scratch home/credentials were removed afterward.
This verifies live native continuation, not preservation of the original inference.
