# Headless nightly parity

The active target is T3 Code headless nightly **4275**, commit
`6577c6b6185167c0e502fa01f9e8e8f2dee6ff25`, published on 2026-11-06.
[Upstream release](https://github.com/pingdotgg/t3code/releases/tag/v0.1.8-nightly.20261106.4275).
The implementation is not yet a complete 1:1 replacement.

The scope includes every headless RPC, command, HTTP/CLI surface, authentication
flow, stream lifecycle, provider driver, native continuation, shared-state transition
and observable error/cancellation behavior. Methods without a current client caller
remain in scope. Legacy aliases are tracked separately and do not count as current
nightly coverage. Pure web/mobile/desktop presentation is outside this headless target;
server behavior supporting those clients remains in scope.

## Reference and evidence

[`nightly/parity-4262.json`](nightly/parity-4262.json) inventories 211 discovered
public method names and records existing method statuses as historical evidence,
not certification against this nightly. The source adds two previously untracked
methods: `orchestration.getThreadTitles` and `sourceControl.star`. Both are now
tracked and implemented in their documented scope.

[`nightly/drift-4275.json`](nightly/drift-4275.json) records the next intake. Goal,
thread-read and RPC authorization source is unchanged from 4262. Device discovery,
Antigravity identification and CLI/service-launch changes require their own comparison;
existing 4262 runtime evidence does not certify those surfaces.

The reference's shared database has migration head 80. Live O3 profiles are accepted
through migration 73. Supporting migrations 74–80 requires isolated native-T3 migration,
reducer/export/import and schema validation before widening the live-profile gate.
The existing migration guard remains the protection against opening an incompatible
profile. Preserving a historical status of `done` does not bypass this acceptance.

For each implementation block:

1. Identify the exact source functions, schemas and lifecycle, including errors and races.
2. Run pure upstream functions when possible and retain reproducible golden cases.
3. Drive the real compatible client through the server and native protocol peers.
4. Exercise installed native dependencies for behavior a peer cannot establish.
5. Check restart, Stop, stale/duplicate events, ownership and resource release.
6. Record evidence and remaining gaps; only claim parity for the behavior actually checked.

## Current execution block

The native-goal block replaces Codex slash-command prompts with native goal APIs,
retains automatic turns within their owning app run, pauses uncontinued goals after
the upstream five-second grace, and stops late orphan turns. Control-only turns have
no native rollback reference. Claude goal acknowledgements and synthetic Stop-hook
feedback update the provider-thread goal; child output and failed turns cannot mark
a root goal complete. Golden fixtures execute the nightly's actual parser and goal
normalizers. This block does not close the broader idle-wake/child-continuation,
provider-driver or shared-migration backlog.

Codex goal notifications received before a resumed thread is subscribed are now
retained in a bounded cache. Native state takes precedence over a missing or stale
stored goal, including an explicit clear, without adding goal RPCs to ordinary
messages or treating unknown children as owned roots. Activation acknowledgements
also share the notification queue, preventing a queued preparation update from
overwriting a successful activation after a fast first turn. Restart acceptance
beyond these boundaries remains part of the native-session backlog.

The thread-read block adds title batches, active family rows and exact-path worktree
membership. An isolated official nightly 4262 runtime agrees on selection, ordering,
Unicode trimming and 31 malformed-input wire defects. Families reuse the common shell
projection, whose complete field fidelity remains a separate task. The full headless
goal remains active after either block.

The live-history repair retains the upstream active-run message cohort and orders
visible windows by canonical position rather than physical record age. Plain and
compact compatible-client snapshots preserve a growing answer through600 tools,
and older pages remain complete without duplicates. This repair is installed on
NODE and M2. The two reported NODE threads expose readable answers and active goals,
and both executions continued after restart. Intel remains unreachable.

Server teardown now fences late provider receipts without issuing user Stop or
native goal-control commands. Process acceptance includes graceful and abrupt
restart; complete session-disposal, idle-wake and forced-stop parity remain open.
See [`nightly/shutdown-intake.json`](nightly/shutdown-intake.json).

The forge-star block provides personal GitHub GET/PUT operations, host-scoped
credentials, native private token slots and quota protection. The real compatible
client exercises read/star/read against private HTTP, while the official runtime
matches fallback, decoding and permission responses. App sign-in, secret providers
and policy integration with the remaining forge consumers remain open; see
[`nightly/forge-star-intake.json`](nightly/forge-star-intake.json).

Validation details belong in `CONFORMANCE.md` and the per-block intake files. A green
suite and an RPC count alone do not establish complete headless behavior.

The native-agent isolation block adds Linux resource scopes, per-profile orphan
cleanup, OOM diagnosis and shared Codex protection. Source helpers agree on 163
executed cases, and private peers exercise configured native transports and the
compatible client's OOM response. A real systemd user manager in a disposable
container also verifies detached-child cleanup, shared-process protection, a
kernel OOM at 32 MiB and orphan cleanup across two profiles. `forceStopThread` orchestration remains open;
see [`nightly/agent-scope-intake.json`](nightly/agent-scope-intake.json).

The server-browser launcher now uses that registry too. The real systemd case
verifies inherited CDP descriptors, scope membership, child OOM score, shared
profile lifetime and shutdown with a private protocol peer. Real Chromium
rendering remains covered only by its separately documented acceptance.

Interactive and managed setup PTYs now use the general collecting agents slice,
with clean environments, preserved wrapper argv and child OOM adjustment. Real
systemd checks verify controlling TTY and PID/session/group identity, lazy API
initialization and survival of owned-profile agent cleanup. This does not add
durable PTY reconnection after an actual server restart.

Failed turns now publish a structured terminal error card with upstream identity,
bounded and redacted messages/codes, class, retryability and optional usage reset.
Confirmed OOM changes the message and code while retaining the failure class.
The real client renders the card; stale attempts and duplicate terminal events
cannot replace it. Provider-specific classification, retries and the remaining
session lifecycle are still open; see
[`nightly/provider-failure-intake.json`](nightly/provider-failure-intake.json).
