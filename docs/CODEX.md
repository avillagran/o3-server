# Codex app-server adapter

`CodexProvider` in `src/codex.rs` runs one `codex app-server --stdio` process for up to
32 concurrent native threads. Production startup probes initialize → initialized,
model/list (first 100 models), account/read. `server.getConfig` reports the native
model names and authentication result, or an explicit error if spawning/probing fails.
No authentication or installation flow is advertised. The executable installs this
adapter; a multi-provider router can compose its `Provider::config` catalog.

The protocol is pinned to installed **codex-cli 0.166.0**. The relevant generated
schemas are checked into `tests/fixtures/codex/protocol-0.166.0.json`; generation uses
`codex app-server generate-json-schema --experimental --out DIR`. Stdout carries
newline JSON-RPC without Content-Length or a required jsonrpc member. Stderr is
currently discarded; native RPC errors and turn failures remain visible.

Native thread bindings persist on the app thread in SQLite. Each app thread has a
provider-session binding to the shared process. Subsequent runs use thread/resume
with excludeTurns=true: no history is hydrated into adapter RAM. Codex rejects
resume of an empty thread before any rollout exists; only that specific error
falls back to thread/start. Native turn IDs, item refs, requests, root nodes and
attempts are projected. Controls implement turn/steer with expectedTurnId and
turn/interrupt. Runtime requests namespace app IDs by run and retain the original
JSON ID value for replies, including numeric IDs.

Command/file approvals, structured user questions and MCP elicitation responses
are supported. Approval nodes and renderable approval/question items are persisted.
Control capacity is reserved before committing steering or live approval responses;
missing/busy channels fail rather than silently accepting a lost response. This
still is not a durable outbox: a server crash after commit can lose delivery.

Rendering includes assistant text, reasoning parts, command lifecycle/output/exit,
file changes/diffs, MCP/dynamic tool results, todo plans and proposed plan text.
Token notifications update provider-turn context usage; rate-limit notifications
update the provider session and the getConfig usage-limit windows. Completion keeps
usage already received, closes streaming items and finalizes execution metadata.

Memory bounds: 32 actor commands/outstanding RPCs/active thread routes, 16 native
frames and 16 supervisor events per active run, 8 controls, 16 pending native
requests per turn. Frames and assistant bodies are limited to 1 MiB; streamed tool,
plan and reasoning bodies to 256 KiB. Slow native consumers fail closed and interrupt
their turn instead of growing an unbounded queue. One partial frame survives select
cancellation. No transcript collection or history cache lives in the adapter.
The process is killed and reaped on actor exit; `shutdown()` explicitly waits for
reaping. The child is outside the server RSS budget; this change does not establish
a new RSS measurement or a <50 MiB bound under concurrency.

## Evidence and remaining gaps

- `real-0.166.0.ndjson` records real initialize, account/model queries, thread/start,
  empty-thread resume failure and invalid-turn interruption frames in an empty
  temporary CODEX_HOME. Only the temporary path was normalized. No inference call
  or real T3 profile was used.
- `notifications.synthetic.ndjson` contains schema-shaped synthetic turn traffic.
  It verifies SQLite reduction for tools, files, reasoning, plans, usage and completion.
  `mock-server.py` replays real startup results and synthesizes active turns.
- Unit tests cover concurrent multiplexing, native request identity, steering,
  resume, interruption, framing bounds and partial reads. `tests/codex_contract.rs`
  traverses the real Connection, dispatch, stdio peer and SQLite projection.
- The ignored live test can be run with `cargo test --lib
  live_installed_codex_handshake_and_empty_resume -- --ignored`. It uses an empty
  temporary CODEX_HOME and checks actual initialization, thread creation and the
  empty-thread resume error; it performs no authenticated inference.

Attachments, additional message context, auto/auto-accept-edits runtime policies,
subagent/background task settlement, dynamic tool callbacks, provider-native rollback/fork,
provider-instance customization, setup/auth UI, durable control retry and active
turn recovery after server restart remain unsupported. The two supported runtime
modes are approval-required and full-access; plan interaction mode and selected
reasoning effort are passed natively. Unsupported input is rejected explicitly.
Live authenticated tool execution/approvals and successful persisted-rollout resume
have not been exercised; their behavior is covered by the deterministic peer and
schema pin only. Full upstream parity is not claimed.

Integrated main now routes Claude/Codex through one registry, emits command-output
events, promotes queued input to native steering, and captures/restores Git workspace
checkpoints. Native conversation rewind remains unsupported. Current integrated
verification and RSS are recorded in IMPLEMENTATION.md and rss-measurement.json.
