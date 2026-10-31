# Core implementation checkpoint

Implemented and exercised through the real `t3-protocol::Connection`:

- CLI/profile bootstrap, local session issuance, readiness helpers, live-PID refusal and owned
  runtime cleanup. `service install --dry-run` is tested; actual systemd installation is not run.
- Discovery, one-use OAuth pairing, session state, pairing/client management, scope failures,
  and revocation of open sockets. WebSocket protocol-2 rejection precedes authentication.
- Multiplexed/batched RPC, Ping/Pong, one unacknowledged chunk per subscriber, Interrupt/Eof,
  at most 64 subscriptions per socket and database replay instead of event queues.
- Settings and unknown keys on disk, secret redaction/sentinel preservation, config snapshots.
- SQLite shared event sequence, atomic command receipts/projections, project creation, thread
  lifecycle, messages, runs, turn items and minimal runtime requests. Restart retains receipts.
- Shell/thread snapshots, resume and synchronized markers, bounded reads and history pages.
- Injected scripted provider start/text/approval/completion, response, interrupt and dismissal.

OmaBridge preparation is implemented, not the bridge itself: `State: Send + Sync`, `Arc`, SQLite
mutex, two workers/two blocking threads/512 KiB stacks. `serve_io` accepts any `AsyncRead +
AsyncWrite + Unpin + Send + 'static` transport; TCP and authenticated DuplexStream HTTP/WS
have tests. No `Rc`, `RefCell`, `LocalSet` or local spawning remains in the server.

## Remaining gaps

This checkpoint is **not yet a complete drop-in replacement** for the desktop/Android clients.
The compatibility registry marks broad methods partial where appropriate.

- Existing T3 signing keys and environment identity are reused, but existing session rows are
  not imported from T3's database. A prior T3 bearer needs reissuance; key reuse alone does not
  preserve its session. Neither T3 data nor its running service was accessed during this work.
- Settings implement a minimal static default catalog and common secret field redaction,
  not every upstream schema annotation/migration/validation rule. Provider-instance mutations,
  exact per-field patch semantics and every typed settings failure remain. Config changes send
  replacement snapshots, not the complete incremental event union.
- Shell snapshots currently assemble JSON in memory and cap projects/threads at 10,000.
  Thread projections read at most 100 records of each kind; the subscriber always gets the
  bounded shape, even without bounded opt-in. Implement streaming serialization, a complete
  opt-out snapshot and proper large-shell handling before claiming the memory/design contract
  fully met. There is no retained transcript cache or unbounded subscriber event queue.
- Advanced node/session/turn/attempt/plan/checkpoint/provider projections are not reduced.
  Approval rendering and durable provider/control recovery require the work in PROVIDER.md.
  A crash after dispatch commit but before spawning leaves a preparing run. Control sends can
  fail after a receipt commits. These are correctness gaps for real provider execution.
- Launch supports new root-workspace threads; worktree creation, existing-worktree/reuse and
  one atomic launch transaction are unfinished. Its create and initial-message stages have
  separate deterministic receipts. The fake is injected only in tests, never enabled by the CLI.
- Dispatch modes defer_start, start_immediately and queue_after_active are accepted. Steering
  (steer_active, promote-to-steer), restart_active, deferred release and unlisted lifecycle commands
  are not implemented.
- Auth implements bearer sessions, not DPoP/cookies. OAuth request validation/scope negotiation,
  complete HTTP error unions, and live connection metadata in the clients listing need work.
- No conformance run against upstream T3, desktop UI or Android was performed. Tests use the
  real client library with o3-server. No actual systemd installation was tested.

## Terminals, command output, thread commands and operations

All of it is exercised through the real `t3-protocol` client (`tests/terminal.rs`,
`tests/command_output.rs`, `tests/thread_commands.rs`, `tests/ops.rs`; shared harness in
`tests/support`). Modules: `pty` (libc `openpty`, session leader, process group), `terminal`
(manager, sessions, streams), `streams` (live subscriptions plus the wake-up channel),
`command_output`, `thread_commands`, `ops`, `scheduled`, `routes` (the unary dispatch hook).

- **Terminals.** One task per live process reads the PTY, decodes UTF-8 incrementally and
  appends to a per-terminal scrollback (512 KiB / 5000 lines) and a 192 KiB replay log. Subscribers
  keep a sequence cursor, not a queue; one that falls behind the log is sent a fresh snapshot.
  Each stream allows at most 8 unacknowledged chunks or 64 KiB, as `OutputProtocol.ts` does. Close,
  restart and replace send HUP and TERM to the process group and KILL after 1 s. Gaps: history is
  not persisted across a server restart, `providerInstanceId` environment is not applied, and
  there is no per-terminal log file.
- **Command output.** `ProviderEvent::Command(CommandEvent)` is the Provider-trait side. Output
  only updates a bounded tail; the item is persisted at start and finish. No shipped adapter emits
  commands yet; `ScriptedCommandProvider` does in tests. The tail is plain text, not a folded screen.
- **Thread commands.** Own receipts and transaction in `thread_commands`. The run queue is real
  (`queue_after_active`, positions, hold on `run.interrupt`, advance when a run ends). Steering is
  refused. Rollback, subagent stop and fork record state but cannot act on a provider.
- **Operations.** Honest minimum: processes come from /proc, usage is empty, provider refresh is
  the configured list, cleanup removes only old profile logs. The scheduler is one task, woken by
  task changes; fixed-time schedules are UTC and webhook schedules are refused.

## Verification and memory

`CARGO_TARGET_DIR=/var/tmp/o3-build/<name> cargo test` runs the auth/terminal/scheduler unit tests, 14
real-client integration tests, 4 CLI subprocess tests, 1 embedded-transport test and the terminal,
command-output, thread-command and operations suites. All profiles are temporary.
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are the static checks.

Run `tools/measure-rss /tmp/o3-target/release/o3-server` after a release build. The probe uses a
fresh temporary profile, seeds 1,000 messages (~1 KiB text each) through authenticated RPC,
and opens one live bounded thread subscription. It reports Linux smaps RSS, not heap-only RSS.
The results are in `rss-measurement.json`; no provider process, TLS or OmaBridge is included.
The workload is a single sample, not a sustained/concurrent load test or a peak-memory bound.
