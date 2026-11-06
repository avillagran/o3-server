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
The environment capability is deliberately not advertised yet. Native CLI
acceptance, differential checks, and shared-state validation remain required;
this is not a claim of complete Nightly parity or a deployed feature.

## Behavior

- `message.dispatch.freshContext` accepts a boolean. True is retained on the run
  and its user-message turn item; older commands keep their previous behavior.
- A fresh-context send cannot steer or restart an active turn. It queues, retains
  its selected model/provider, and waits for the previous execution to finish.
  Admission creates a new provider-thread row and native pool generation.
- The previous session is released through the durable provider-switch queue
  before executing new provider work. Imported native bindings without a modern
  provider-thread row are also released. Failure to detach prevents execution;
  subsequent work retries the pending release.
- Background agents/commands and pending fork/merge transfers prevent a reset.
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

Eight focused tests exercise the real O3 command transaction and provider
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

## Acceptance still required

- Real Claude and Codex session reset/resume, including post-reset provider
  switching, failure recovery, and preservation of messages visible to clients.
- Differential run/turn-item/session events against T3 4262.
- Shared-state acceptance on a disposable database copy and validation with T3's
  own schemas. Never change `MIGRATION_HEAD` merely to enable this feature.
- Advertise `environment.capabilities.freshContext` only after these gates pass,
  then verify the O3 desktop composer against O3 Server. The desktop work is on
  `omarchy-t3-code` branch `feat/nightly-4247-parity`.
- Scheduled-task fresh-session execution is a separate remaining integration.
