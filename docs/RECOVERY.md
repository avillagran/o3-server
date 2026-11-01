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
Active runs fail with a restart error; active inference and approvals are not resumed.
This is a durable handoff journal, not exactly-once native delivery or a replay worker.
The native protocols do not expose an idempotent command acknowledgement across
restarts. Records are retained in SQLite; retention/GC and frontend presentation of
the new error metadata remain future work. Never claim full native chat parity.

Launch checkpoint: the thread, initial message/context/run, event rows and launch
receipt now share one transaction. Native execution starts after commit. A failed
run insert leaves none of those records; retrying the same launch creates one run.
This improves database atomicity, not crash replay of committed native starts or
filesystem rollback of a newly prepared worktree.
