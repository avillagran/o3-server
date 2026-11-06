# Nightly 4247 fresh-context execution

Introduced in T3 Code `v0.1.8-nightly.20261106.4247`, commit
`e136e8e3d4cb85a56463d449f1d3835313737e43`.
Current overall target: [Nightly 4262](https://github.com/pingdotgg/t3code/releases/tag/v0.1.8-nightly.20261106.4262),
commit `c55db3e9c72fdf584f4b54192fb39beb88667b59`.
The source diff from 4247 to 4262 leaves fresh-context dispatch, boundary,
queued admission, `RestartBackgroundNote.ts` and `ProviderTurnStartService.ts`
unchanged. New receipt replay and queue-resume changes remain separate parity work.

Status: execution and history-boundary implementation on
`feat/nightly-4247-fresh-context`, based on `dev` at `5abc0d9`.
The candidate now advertises `environment.capabilities.freshContext` after the
native CLI, source-policy and shared-schema checks below. This is not a claim of
complete Nightly parity or a deployed feature. The wider migration-80 and
cross-server acceptance backlog remains open.

## Behavior

- `message.dispatch.freshContext` accepts a boolean. True is retained on the run
  and its user-message turn item; older commands keep their previous behavior.
- A fresh-context send cannot steer or restart an active turn. It queues, retains
  its selected model/provider, and waits for the previous execution to finish.
  Admission creates a new provider-thread row and native pool generation.
- A previous single-thread session is released through the durable
  provider-switch queue before executing new provider work. Multi-thread sessions
  remain alive and receive a new native thread, following the native policy. Imported native bindings without a modern
  provider-thread row are also released. Failure to detach prevents execution;
  subsequent work retries the pending release.
- Background agents/commands prevent releasing their session; a multi-thread
  session can remain alive while another native thread starts. Pending fork/merge
  transfers prevent a reset.
  Command rejection rolls back its message, run, ownership changes, and receipt.
- Queued resets do not establish a context boundary or release the active
  session. A deferred reset cancelled before starting restores the previous
  binding at the next admission, including after a server restart. Binding snapshots live in an O3-only SQLite
  table, not in T3 thread payloads.
- Visible messages and turn items remain unchanged. History supplied implicitly
  to providers is restricted to the latest admitted fresh-context boundary.
  Older provider threads are excluded from reusable switch targets; imported
  context, attachment handoffs, ancestor fork history, retry handoffs and native
  recovery cannot replay material before that boundary.
- Recovery of a fresh run before native input acceptance creates no artificial
  history handoff. Its cleanup and replacement binding still use the durable
  native-recovery receipt.

## Reference evidence

The implementation follows these functions at the exact reference revision:

- `apps/server/src/orchestration-v2/Orchestrator.ts`:
  `contextBoundaryOrdinal`, `providerThreadPredatesBoundary`, fresh-context
  dispatch, queued admission and session-release paths.
- `apps/server/src/orchestration-v2/RestartBackgroundNote.ts`:
  `neverStarted` and `cancelledBeforeStart`.
- `packages/contracts/src/orchestrationV2.ts`: optional run/command
  `freshContext` and the user-message turn-item marker.

O3 reuses its existing durable detachment and native-recovery mechanisms.
The change does not advance the shared T3 migration head or alter T3's SQL schema.

## Validation

Ten focused tests exercise the real O3 command transaction and provider
supervisor with an in-process provider probe:

1. Reset closes the old session before execution; future provider switches and
   return navigation cannot reuse the old context or native row.
2. A queued reset, including a provider change, leaves the running turn alone.
3. Queued and cancelled unstarted runs do not establish a boundary; prepared
   restart continuations retain the reference exception.
4. Background work and pending transfers reject atomically.
5. Cancelling a deferred reset preserves the previous session for the next send
   across a server restart.
6. Detach failure blocks execution, and the next send retries cleanup.
7. History readers and native recovery exclude pre-reset history while keeping
   the visible messages.
8. A legacy imported binding is released before a fresh run on another provider.
9. Sixty golden cases execute the exact upstream boundary, provider-row and
   session-release functions: 32 boundary cases, 12 thread cases and 16 sessions.
10. A multi-thread session keeps its background work alive when resetting only
    the native thread.

The new tests create unique temporary profiles, do not remove filesystem trees,
and do not launch shells or native provider processes. They establish supervisor
and transaction behavior, not external CLI compatibility.

Commands:

```sh
cargo test --no-default-features --lib fresh_context::tests
cargo test --lib fresh_context::tests
cargo clippy --no-default-features --lib -- -D warnings
cargo clippy --lib -- -D warnings
cargo fmt --all -- --check
```

## Native and shared-state acceptance

`tools/live-fresh-context` compiles without executing on the host and runs the
opt-in integration in rootless Podman. Code, binaries and credential inputs are
read-only mounts; provider profiles live in tmpfs. Only a unique private evidence
directory is writable. The runner stops only its own uniquely named container.

Eight real turns with Claude Code 2.1.318 (Haiku) and Codex 0.167.0
(`gpt-6.1-sol`) prove original-context recall, forgetting after each reset, new
context continuity, post-reset provider switching, changed native identities and
retained visible history. The final O3 run passed after applying the upstream
multi-thread session policy. The identical eight-turn scenario also passed against
the official T3 4262 Linux binary inside the same container boundary. All seven
behavioral results match. Its migration-80 database has 109 public records and
264 events, all accepted by the exact contracts with zero rejections.

The official archive SHA-256 was verified against its GitHub release digest:
`5df94ca15c4f68f749fa5540fbb9d5ca879ca8750ac43dd2704f6a18899ed6d5`.
The reference uses an isolated profile and disposable authentication session;
neither server runs against a live user database.

The same native evidence was exported into a disposable copy of the T3
migration-73 schema through the guarded `t3_shared::open` and real exporter:
81 records, two fresh runs and sixteen messages. Nightly 4262's own contracts
accepted every public row and all 80 exported events with zero rejections, using
its exact Effect 4.1.2 runtime. The validator refuses a mismatched installed
runtime before decoding, and rejected rows now produce a failing exit code.
This establishes that additive fresh-context data fits the currently supported
shared schema; it does not establish support for migration head 80.

The validator exposed a pre-existing context-cost difference: bare USD numbers
were converted only on shared export. Snapshots, new events, imported usage and
older replay data now retain `{amount, currency}`. An idempotent startup repair
runs once; turn accounting remains unchanged. Two focused tests cover currency
preservation and legacy snapshot/event repair. The earlier rejected evidence is
retained as a negative control: the validator now exits nonzero on rejection.
The existing provider-input peer regression also passes in a credential-free,
network-disabled container.

Reproduction:

```sh
node tools/generate-fresh-context-fixtures.mjs /path/to/t3code
# Rootless container, real short native turns:
tools/live-fresh-context
# Same scenario against the extracted official reference package:
tools/live-fresh-context --reference-root /path/to/t3-0.1.8-nightly.20261106.4262-linux-x64
# Credential-free, network-disabled container, using that run's synthetic DB:
tools/live-fresh-context --export-database /path/to/evidence/o3.sqlite
T3_CONTRACTS=/path/to/t3code-4262/packages/contracts AFTER_SEQUENCE=0 \
  node tools/validate-t3-shared.ts /path/to/export/evidence/statev2.sqlite
```

## Remaining integration


- Validate migrations 74–80 and bidirectional runtime handover on a disposable
  profile before advancing `MIGRATION_HEAD`.
- Verify the O3 desktop composer against the enabled candidate. The desktop work is on
  `omarchy-t3-code` branch `feat/nightly-4247-parity`.
- Scheduled-task fresh-session execution is a separate remaining integration.
