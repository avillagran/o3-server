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
  entry-map replacement/removal and redacted environment retention are tested;
  full model-preference/background-activity normalization and all validation annotations remain. Config changes send
  replacement snapshots, not the complete incremental event union.
- Shell and thread snapshots now stream from a separate read-only SQLite transaction through
  two 16 KiB channel slots; WebSocket continuations carry one RPC JSON message, and HTTP uses
  streamed bodies. Full subscriptions have no 100-record cap and shell has no 10,000-record cap.
  Compact IDs use a capped dictionary, message text is deduplicated against included messages,
  and local visible items can be omitted from turnItems. Ten-turn/75-generic/500-item/1 MiB
  history windows share a policy; conversation/activity filtering, throughEntryId, older-page
  widening and oversized-body transport truncation still need upstream conformance.
- Advanced node/session/turn/attempt/plan/checkpoint/provider projections are not reduced.
  Approval rendering and durable provider/control recovery require the work in PROVIDER.md.
  Runs now record execution intent, so restart can fail executions stranded before spawn.
  Control sends can still fail after a receipt commits; durable provider control replay remains.
- Launch supports root, existing_worktree, git worktree creation and reuseExistingThread.
  Empty launches are idempotent. Create and initial-message stages still have separate receipts;
  atomic initial-message rejection, complete workflow cleanup and reused metadata remain.
- Restart atomically marks orphaned executions failed, cancels pending requests and marks live
  provider sessions disconnected, with replayable events; explicitly deferred runs survive.
  Automatic provider-session reconnection/control replay is not implemented. This is safe
  terminal recovery, not continuation of provider execution. Recovery runs once at TCP serve
  startup; embedded hosts must call `recovery::recover` before accepting their first transport.
- Only defer_start and start_immediately dispatch modes are accepted. Queues, steering,
  restart, deferred release and unlisted lifecycle commands are not implemented.
  A crash after dispatch commit but before spawning leaves a preparing run. Control sends can
  fail after a receipt commits. These are correctness gaps for real provider execution.
- Launch supports new root-workspace threads; worktree creation, existing-worktree/reuse and
  one atomic launch transaction are unfinished. Its create and initial-message stages have
  separate deterministic receipts. The fake is injected only in tests, never enabled by the CLI.
- Claude now supports queue_after_active and steer_active; idle model/mode commands persist
  and apply on the next turn. Queue editing, restart and deferred release remain unimplemented.
- Auth implements bearer sessions, not DPoP/cookies. OAuth request validation/scope negotiation,
  complete HTTP error unions, and live connection metadata in the clients listing need work.
- No conformance run against upstream T3, desktop UI or Android was performed. Tests use the
  real client library with o3-server. No actual systemd installation was tested.

## Import and verification

`o3-server import --from /path/to/statev2.sqlite --base-dir /isolated/profile` opens the
source read-only and uses the SQLite backup API to create a private mode-0600 copy under
`/tmp`. Only that consistent copy is queried. Projects, v2 threads/messages/turn items and
local visible items load transactionally into o3.sqlite. Existing local IDs win on reimport.
The synthetic fixture uses verbatim Appendix D migration 005/055 DDL. Tests prove WAL content
is included, source DB/WAL bytes remain identical, rollback on schema failure and CLI idempotence.
No real T3 database was accessed. Legacy v1 thread migration and inherited visibility are not
implemented; imported projections do not reconstruct the upstream event log/provider bindings.

`CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/o3-core-4d71 cargo test --tests`
passes 31 tests, including real-client transport and compact decoding, restart recovery,
worktrees and synthetic import. `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`
and `git diff --check` are the static checks. No systemd service or actual provider was exercised.

`tools/bench-rss` builds release by default (private target under `/tmp`); pass an existing
release binary to avoid rebuilding. It uses a fresh on-disk `/var/tmp` profile, 20 threads,
1,000 messages of ~1 KiB, three simultaneous compact bounded subscribers and 1,000 unary
settings reads. It reports kernel VmRSS, VmHWM and RssAnon in KiB at each phase. The measured
binary hash and workload are recorded in `docs/rss-measurement.json`. The measured workload is
below 50 MiB VmHWM without purge tuning; SQLite writer cache is 2 MiB and snapshot cache 512 KiB.
Provider subprocesses, OmaBridge, TLS and sustained slow-client load are excluded.
Run `tools/measure-rss /tmp/o3-target/release/o3-server` after a release build. The probe uses a
fresh temporary profile, seeds 1,000 messages (~1 KiB text each) through authenticated RPC,
and opens one live bounded thread subscription. It reports Linux smaps RSS, not heap-only RSS.
The results are in `rss-measurement.json`; no provider process, TLS or OmaBridge is included.
The workload is a single sample, not a sustained/concurrent load test or a peak-memory bound.

## Claude implementation update

The executable now registers a direct native Claude CLI provider when available. See the
2026-10-31 checkpoint in PROVIDER.md and tests/fixtures/claude/README.md for observed protocol,
verification and remaining limits. The earlier core-only test count and RSS measurement above
remain historical; they do not measure this adapter. No production T3 service/profile was used.
