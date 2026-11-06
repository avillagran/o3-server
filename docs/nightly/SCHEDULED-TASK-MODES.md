# Scheduled conversation modes at nightly 4262

Reference: `c55db3e9c72fdf584f4b54192fb39beb88667b59`.
This block builds on the fresh-context and migration-80 candidate and merges dev
through PR #48 (`3102911`), retaining native goal ownership and continuation.

## Implemented behavior

- `new_thread` leaves the task unbound and launches a separate thread each time.
- `continue` and `fresh_session` bind their first successful launch. Subsequent
  runs queue behind active work in that thread and use its current provider and
  model. Only `fresh_session` sets `freshContext` on bound dispatches; the first
  message of a newly launched thread does not need a reset.
- Saves preserve a mode and a first-run binding omitted by a stale client. A
  legacy save with an explicit thread derives `continue`. Explicit `new_thread`
  releases the binding. Moving projects releases the previous project's thread
  unless the caller selects a different thread.
- Messages and user turn items retain `scheduledTaskId`, creator and creation
  source. Initial launches forward the same metadata. MCP authority continues to
  force its own actor/source, and invalid creation fields are rejected atomically.
- Dispatch admission is reserved per task and released on every return path.
  Completion uses the original row's creation time and start time, preserves
  concurrent edits, never resurrects a deleted task or stamps a replacement, and
  cleans up a thread deleted before its first binding could be stored.
- Legacy task views derive their mode without changing their prior meaning.
  `environment.capabilities.scheduledTaskThreadModes` enables the desktop selector.

## Evidence

`tools/generate-task-mode-fixtures.mjs` executes the exact reference
`scheduledTaskThreadMode` and `resolveThreadMode` functions. The committed fixture
contains 108 save cases and eight legacy/stored cases, with revision and source
hash. It does not substitute a rewritten reference function.

Build the dependency-complete local runtime once with `tools/build-test-image`.
It contains Python, Git, Node and FFmpeg without copying source or user profiles
into the image. Test containers mount source and compiled artifacts read-only;
they receive no credentials or network access.

`tools/test-scheduled-modes` compiles without executing tests on the host and runs
in rootless Podman with read-only source/binaries, private tmpfs state, no network
or credentials. It covers real-client mode/binding/queue/provenance behavior,
completion/deletion/replacement state races and the existing scheduling, webhook,
MCP launch, fresh-context and native-goal regressions. Add `--default-features`
to exercise the integrated default feature set. Both configurations pass 60
selected regressions, with three native-goal opt-ins left ignored. The shared
history suite also passes its 14 regressions. Production and acceptance Clippy
checks pass in both configurations.

The full GitHub Actions Rust matrix now runs inside the official
[Rust container](https://hub.docker.com/_/rust) and checks `/.dockerenv` before
testing. This uses GitHub's documented [job container](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/run-jobs-in-a-container)
isolation; local native and regression runners retain their rootless Podman
boundary. Container-only regressions accept either runtime marker rather than
skipping coverage on CI.

`tools/live-fresh-context --scheduled-tasks` drives six real Claude Haiku turns:
first binding, same-session recall, reset/forgetting, post-reset recall, and two
independent unbound launches. It asserts the bound thread keeps its Claude
provider even after the task's stored selection changes to Codex, checks native
identities, keeps eight visible messages on the bound thread and retains task
provenance. Source, binaries and credential inputs are read-only; provider profiles
live in container tmpfs. Only a unique private evidence directory is writable.

The identical scenario completes against the official T3 4262 binary with
`--reference-root <extracted-package>`. Earlier reference attempts returned
inconsistent inference answers (`READY` on recall, or `UNKNOWN` after a reset)
despite completed turns and retained native identities. Those failures remain
evidence; their cause is unproven, and one successful run is not a stability
certification. Per-stage projections and selected synthetic native transcript
text are captured for diagnosis without copying provider credential files.

## Webhook recovery correction

The full CI matrix exposed concurrent workers reclaiming the same persisted
webhook claim while one worker was still dispatching it. The second worker hit
the task admission reservation and postponed a delivery that had already been
admitted. A per-delivery owner now excludes live work from both claims and due
wakeups. Releasing that owner wakes the scheduler; a persisted claim with no live
owner remains immediately recoverable after interruption. Only a selected job's
prompt is read, avoiding payload scans through other workers' claims.

The existing crash/receipt regression reproduced the failure before the fix and
passes afterward. A second regression verifies live ownership, release and
recovery directly. Both are now included in the routine container runner;
`--unit-filter webhook::crash_tests` executes them without rebuilding unrelated
integration binaries. Use `--integration <target>` for an affected integration
crate or `--all-tests` for every ordinary Cargo test. The runner mounts the real
CLI artifact at its compiled path and starts in the checkout, matching CI
workspace authorization. Complete output is retained in a unique private log.

The PR merge matrix for head `7602e9c` passes: 533 minimal and 536 default tests, 37
ignored opt-ins per configuration, all-target Clippy, formatting, compatibility
generation and watcher checks. This is evidence for that commit, not a claim
of full-platform parity. GitHub tests the candidate merged with `dev`, which
includes the thread-read additions from PR #49. The standalone candidate before
that merge has 530 minimal tests and 36 ignored opt-ins; these counts must not be
reported as equivalent checkout contents.

## Still open

The credential-free official-runtime store proof and the remaining handover
requirements are documented in [SHARED-SCHEDULED-STORE.md](SHARED-SCHEDULED-STORE.md).

- Diagnose the reference's intermittent rapid-follow-up answers; avoid treating
  a successful repeat or a guessed cause as resolution.
- Native webhook ingress/dispatch, completion recovery and lossless migration of
  old accepted O3 work. Definitions and native ledger rows now pass bidirectional
  storage checks; preserved pending rows alone do not prove execution handover.
- Complete service lifecycle comparison, including thread/project moves,
  timer/manual/webhook contention, restart recovery, managed policy and failures.
- The live desktop composer, remaining RPCs/drivers and full platform audit.

The O3 native scenario's 72 public records, including two task definitions, pass
the exact 4262 contracts with Effect 4.1.2. The completed reference scenario's 54
bound-thread projection records also pass. An invalid task mode is rejected with
exit status 1. The eight-turn mixed Claude/Codex fresh-context scenario passes
again after merging PR #48.

No live profile or service is changed. The upstream validated desktop pin remains
unchanged while the complete parity goal is still active.
