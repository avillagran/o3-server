# Durable usage-limit continuations (nightly 4301)

Reference: `4cb6945dc60ed03f567dc5daafd083e52308e82b`, especially
`UsageLimitRecoveryWorker.ts`, `ProviderRuntimeRecoveryService.ts`, the three
run-creation branches in `Orchestrator.ts`, and the optional run tag in contracts.

The worker uses native arm/resume command identities, the reset/completion clock,
explicit recovery choices, the one-day overdue arm bound and independent snooze
state. Metadata updates merge the two optional flags only for the same failure,
and rotate their request identity. Automatic arming enters through a private
trusted call path; a public command ID prefix is not authority. A stale automatic
sweep cannot overwrite a committed explicit choice or a Stop that reached the run.

Every newly created continuation preserves `usageLimitContinuationOfRunId`.
Recovery gives only tagged, unstarted active continuations a fresh request ID,
cancels their obsolete copy and marks its user item as a cancelled queued turn.
Already-started and legacy untagged copies do not acquire a fresh identity.
Tagged started work is also excluded from the ordinary native-conversation
restart path; the presence of a native session ID cannot replay that continuation.
Missing start timestamps do not count as proof of an unsent turn.
Receipt-time admission rechecks the source failure, request ID, provider identity,
archive/deletion/settlement state, pending requests and snooze deadline. Duplicate
receipts cannot create another run. A snooze racing the command rotates its nonce
so the worker can send again after the snooze.

The generator executes the actual native worker function with real ID schemas
and Effect DateTime values. Its 760 decisions match the Rust port. A private
integration fixture uses the real HTTP/WebSocket client, durable records/receipts,
recovery entrypoint and an explicitly configured local provider: it observes one
new tagged continuation, answers its fresh synthetic approval, and proves repeated
RPC delivery does not create a fourth run. Started/legacy cases retain two runs.
The restart fixture reuses its listener and explicitly wakes the parked scheduler
as a fresh startup would. A separate real settings-RPC case verifies automatic
arming and immediate scheduler invalidation. Recovery reads root failures through
a partial SQL index, excluding unrelated transcript bodies; settings, relevant
thread commands and usage failures wake scheduling without waking on each token. It does not run a paid provider or claim a literal OS restart.

The broader recovery implementation retains existing behavior for started work;
these checks do not certify every native runtime-reconciliation branch, exact
schema diagnostic trees or arbitrary JavaScript Date.parse strings. A new complete
matrix is required after this checkpoint is integrated. No live profile or service
is migrated by this change.

## Shared schema-80 handover checkpoint

The `usage_continuation_tags_and_recovery_nonce_survive_schema80_handover_without_replay`
regression uses the real project/message reducers, schema-80 shared exporter and
importer, retained SQLite snapshots, and the production recovery entrypoint.
Both minimal and default container modes pass the same case. An unsent tagged
continuation retains its source run across export/import and rotates its recovery
nonce once; an already-started continuation does not acquire a fresh nonce.
Repeated recovery creates no extra run, and the resulting tag/nonce survive the
next export. No provider adapter is installed or run.

This is actual shared-storage/recovery acceptance against the native schema
fixture. It is not an official T3 process executing or resuming the new tagged
turn. The fixture's one regular empty database is renamed to a retained evidence
file before `VACUUM INTO`; no directory tree is deleted or extracted.
