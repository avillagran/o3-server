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
- Only defer_start and start_immediately dispatch modes are accepted. Queues, steering,
  restart, deferred release and unlisted lifecycle commands are not implemented.
- Auth implements bearer sessions, not DPoP/cookies. OAuth request validation/scope negotiation,
  complete HTTP error unions, and live connection metadata in the clients listing need work.
- No conformance run against upstream T3, desktop UI or Android was performed. Tests use the
  real client library with o3-server. No actual systemd installation was tested.

## Verification and memory

`CARGO_TARGET_DIR=/tmp/o3-target cargo test` runs 22 tests: 3 auth unit tests, 14 real-client
integration tests, 4 CLI subprocess tests and 1 embedded-transport test. All profiles are temporary.
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are the static checks.

Run `tools/measure-rss /tmp/o3-target/release/o3-server` after a release build. The probe uses a
fresh temporary profile, seeds 1,000 messages (~1 KiB text each) through authenticated RPC,
and opens one live bounded thread subscription. It reports Linux smaps RSS, not heap-only RSS.
The results are in `rss-measurement.json`; no provider process, TLS or OmaBridge is included.
The workload is a single sample, not a sustained/concurrent load test or a peak-memory bound.
