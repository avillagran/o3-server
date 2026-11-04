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
Runs with Claude/Codex native IDs enter `starting` with durable internal continuation bookkeeping, then continue automatically after provider registration.
This is a durable handoff journal, not exactly-once native delivery or a replay worker.
The native protocols do not expose an idempotent command acknowledgement across
restarts. Records are retained in SQLite; retention/GC and frontend presentation of
the new error metadata remain future work. Never claim full native chat parity.

Staged launch checkpoint (2026-11-04, supersedes the earlier single-transaction
launch): the thread claim/receipt commits first, as upstream does. Initial
message/context/run/events and the preparation job then commit together. A rejected
message admission leaves the empty claimed thread; retry recovers its identity and
admits one message/run without workspace effects from the rejected admission.
The accepted run reads `preparing` while Git and setup execute in the background.
Only release records execution intent and admits the provider.

On restart, an unstarted preparation job can resume its exact durable request.
Potentially started preparation is failed explicitly and cleaned without replaying
an uncertain setup script. A failed bound synchronous setup keeps its checkout and
terminal history for inspection; explicit `prepared-run.retry` reuses that workspace
and the same run/attempt. Pre-release cancellation stops/reaps the setup scope and
removes only owned resources. Failed checkout/ref cleanup stays durable and blocks
retry until it succeeds. The scope ownership guarantee is Linux-specific. This does
not certify native runtime reattachment or cross-instance handoff.

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
The continuation reason stays in the private `run_internal` table after completion;
the pending bit clears on provider start. `executionRequested`, `recoveryPending`
and `recoveryReason` are internal bookkeeping, absent from public run records and
replay events. Existing records, event history and cached launch receipts are
migrated at startup without changing their sequence numbers. Native resume errors fail visibly;
there is no fallback to a new conversation. Missing native IDs fail conservatively.
Tools may have executed before the crash: this does not promise exactly-once side
effects, replay the original user prompt, or recover the old native control channel.

`tests/active_recovery.rs` seeds durable native sessions, starts the actual server
against deterministic Claude/Codex stdio peers, SIGKILLs it during execution, and
starts a second process. Both native IDs are asserted by peers; the same run
completes, internal recovery metadata survives, and the old request is visibly cancelled.
The peers exit on stdin EOF so killed servers do not leave inference running.

Live verification (2026-11-01, NODE): installed Claude Haiku, copied credentials
and native transcript in an isolated `/var/tmp/o3-build` home. A short seed turn
recorded a pending sleep/reply task. The real server resumed it, was SIGKILLed
after the run became running, and a second process resumed the same native
session/run to `completed`. Scratch home/credentials were removed afterward.
This verifies live native continuation, not preservation of the original inference.


## Owned native background work

`backgroundTasks` records retain run/provider-thread/task ownership and terminal
status. Claude task_started/background_tasks_changed and task_notification, and
Codex item started/completed frames, update provider-thread and thread rosters via
ordinary projection events. A native turn result alone cannot settle the run while
owned work remains. Interrupt/failure closes owned records; settlement age never
wins over pending ownership. Duplicate Claude roster starts cannot resurrect an
already completed task within the same run.

Restart marks lost ownership failed with a channel-loss reason, clears its roster,
and closes orphan items/nodes/attempts. It never claims the old command succeeded or
reattaches its process. A bound native conversation may continue in a new turn as
above; background side effects may already have happened and must be inspected.
A reserved provider-thread ID has nativeThreadRef=null until binding and is never
passed as a Claude resume ID. Native IDs resolve through the durable provider-thread
record, with legacy imported-thread fallback; forking uses that same resolution.

## Interrupt recovery audit (2026-11-01)

A live native turn commits an interrupt request while remaining active. Channel
handoff does not mean native success. A native Interrupted terminal event produces
the result and terminal projection together. A 75-second overall control deadline
fails execution with an explicit delivery-unknown error; process death also fails
execution and preserves the control journal as delivery-unknown. The supervisor
checks terminal state while waiting for adapter output so a silent provider is
cancelled after this failure. This bounds server execution state, not proof that
native side effects stopped. The Codex adapter waits for native turn/completed;
Claude waits for the native result boundary after sending its interrupt control.

Restart recovery never resumes a run with an interrupt request. It closes that run
as failed and marks uncertain outbox handoffs delivery-unknown. It does not invent
a successful interrupt result. A run classified as interrupted-before-start has no durable native-turn
acknowledgement and closes immediately with that explicit result message. The
adapter guards prevent an initial send after cancellation observed during init;
cancellation concurrent with native submission does not prove that no native
work began.

When restart continuation is disabled, resume records `cancelled` without a
user interrupt request/result pair or an artificial error. An isolated 3991 restart
with an acknowledged running Codex turn proves the final status, turn items and
normalized timeline/queue reader outputs. The newer RestartContinuation.ts setting
check agrees, but is not the source of this terminal-state choice. Bounded Codex and Claude init/steer/death/restart windows are tested; exhaustive
scheduling and native side-effect cancellation are not claimed.

The legacy internal-field migration now has a durable completion marker and
resumable windows of at most 128 rows and 2 MiB of input per immediate write
transaction (a larger single row is processed alone). Already
migrated profiles do not rescan historical events or reparse cached launch receipts.
A single unusually large record can still exceed a desired time budget; these are
row/byte bounds, not a hard duration guarantee.

### Pinned restart and control-window follow-up (2026-11-02)

The isolated nightly 3991 control-window probe reaches disabled continuation after
an acknowledged, running Codex turn. The reference cancels the run, node, attempt
and provider turn without adding interrupt request/result items. o3 now uses
`cancelled`, removes the artificial disabled-setting error and cancels execution
projections in the same transaction. This supersedes the interrupted-state choice
above; the choice is binary-observed, not inferred from newer source.

The gated peer also covers init before native acknowledgement, steering awaiting
its acknowledgement, provider death after an interrupt request and restart before
the terminal event. Init cancellation must prevent a late turn/start after release
of the native init gate. A queued pre-start interrupt is checked before either Claude sends its initial
user message or Codex starts the native turn. Death/restart without terminal acknowledgement remain failed in
o3 with delivery-unknown outbox state and no synthetic interrupt result. 3991 instead
reports interrupted after provider death and cancelled after restart; these observed
reference states do not establish successful native delivery for o3. This retains
the documented uncertainty boundary and is not original-process reattachment.

### Interrupt budget and retained terminal boundary

The overall deadline measures wall time from post-receipt control-channel handoff
(after the outbox handed-off journal commit) through queued native delivery and
native completion, including owned background work. The timer starts when its
supervisor task runs; it does not wait for native dispatch.
Its 75 seconds cover a preceding Codex RPC (30 seconds), the interrupt RPC
(30 seconds), and 15 seconds for completion notifications. It is an overall upper
bound, including a backlog of controls; it does not prove native side effects
stopped. Terminal runs wake the deadline task so it releases its State immediately.
Quiet supervisors wait on the State change signal instead of polling SQLite.

A terminal result already received remains a known boundary during interrupt.
Once owned tasks/items finish, the adapter reports interrupted without requiring
a second native turn result. Pinned 3991 retains an unresolved runtime request after stopping its run while
the native session stays open. o3 preserves that request too; interruption alone
does not invent a native request resolution. Channel loss/restart cancels it.


MCP launch admission (2026-11-04): caller/key namespaces survive HTTP-session
reinitialization. Both claim and message transactions recheck current authority,
including target modes on receipt replay. A claim committed just before grant
narrowing/revocation remains empty; no deferred run is admitted. Retry after authority
is restored recovers that identity. Accepted replay uses the stored model and does
not require a still-existing base ref or project default.

MCP pending uploads are copied before thread creation, synced and admitted with the
message. Rejected admissions remove this attempt's unused copies and preserve pending
retry sources. Accepted copies are independent from pending deletion/sweeping, and
receipt replay does not create a new set. This does not assert crash-atomic filesystem/
SQLite commits or complete shared non-MCP attachment-intake parity.
