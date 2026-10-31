# Provider boundary

`src/provider.rs` owns the `Provider: Send + Sync + 'static` interface. Implement `execute` as a
boxed `Send` future; install an `Arc<dyn Provider>` in `State.provider` before serving requests.
A routing implementation can select a Claude or Codex adapter from `RunInput.model_selection`.
The executable discovers an executable `claude` on PATH and installs `ClaudeProvider`. Library
hosts choose their own adapter; tests inject `ScriptedProvider`. Unsupported provider instance IDs
and attachments fail explicitly. Claude uses the `claude` instance and `claudeAgent` driver.

The host uses two Tokio workers, two blocking threads maximum, and 512 KiB worker stacks.
`Arc<State>` is shared by TCP and embedded `serve_io` transports. SQLite is protected by a
standard mutex. Never keep that mutex guard across an await or call a State method that locks
it while holding it. All provider subprocess I/O must be asynchronous. Keep CPU-heavy parsing
and blocking work small or use the bounded blocking pool.

## Execution protocol

1. Dispatch commits the command receipt, user message, user turn item and preparing run in a
   single SQLite transaction. Repeated command IDs return the original receipt without executing
   again. `defer_start` persists without starting the adapter.
2. The supervisor supplies thread/run IDs, workspace cwd, initial text, model selection, runtime
   and interaction modes, attachments and optional context. Cwd uses the thread worktree, then
   the project's workspace, then the server's working directory. Never use a profile as a workspace.
3. The adapter awaits sends to a **16-item** output channel. Emit `Started`, `TextDelta`,
   `ApprovalRequest`, then `Completed` or `Failed`. The supervisor commits outputs and notifies
   subscribers; the adapter must not write SQLite or send socket frames.
4. Control uses an **8-item** channel: `Interrupt`, or `Respond` with request ID, decision,
   feedback and answers. A cancellation watch is separate from that channel. Check the current
   cancellation value as well as changes. Output receiver closure also means stop.
5. The adapter owns all children and must terminate and reap them before resolving. Use
   `kill_on_drop(true)` as a backstop. The supervisor allows two seconds to stop after output
   ends, then aborts the task; task abortion alone cannot reap an arbitrary subprocess.

The scripted adapter emits run start, a text delta, one approval, and completion after a
response. Integration tests traverse `Connection` -> dispatch -> provider -> SQLite -> HTTP
projection, as well as interruption/dismissal. No provider process is started in tests.

## Earlier seam limitations (superseded in part by the Claude checkpoint below)

This is a minimal execution seam, not the complete upstream provider event model:

- Add native session/thread/turn identity and resume inputs, setup/auth capabilities, stderr
  diagnostics, tool items, reasoning, plans, usage and provider failure classification. The
  projection store already has record kinds for these; reducers do not yet implement them.
- Approval command text is not projected into a tool item yet; requests currently use the run
  ID as a node identifier and `message` response capability. Create real node/session/turn
  records and an approval turn item before claiming full composer compatibility.
- Scripted approval IDs are fixed for its single-run fixtures. Real adapters must namespace
  request IDs by session/run and translate them to provider-native IDs.
- Persist/recover pending provider effects before supporting crash recovery. Dispatch currently
  commits before spawning; a crash in between leaves a preparing run for recovery work.
- Control delivery currently uses `try_send` after committing a response. Add durable retry of
  undelivered controls; a full/closed control channel must not lose an approval or interruption.
- Queueing/steering/restarting an active run and releasing deferred runs are not implemented.
  Launch currently supports root workspace creation only, with separate idempotent create and
  initial-message stages. Worktree setup and reuse-existing semantics remain to implement.

Do not enable a production adapter merely by advertising it in `getConfig`: provider status and
capabilities must be implemented and tested first. Never collect a transcript in the adapter;
request any needed history from SQLite in bounded pages.

## Direct Claude checkpoint (2026-10-31)

`src/claude.rs` drives the installed native binary directly. Tested protocol version:
Claude Code 2.1.316. Launch uses print mode, stream-json both ways, partial messages,
stdio permission callbacks and an initialize handshake with a 30-second deadline.
A per-thread native process stays alive across results. Up to 16 idle sessions remain;
eviction kills/reaps the process and the next turn resumes the native ID persisted in
SQLite. Models and permission modes use launch arguments initially and native controls
on reuse. Idle thread mode/model commands take effect on the next turn; active changes
are rejected. Each CLI owns an isolated process group; interruption/cancellation kills
that group and reaps the child. Drop is a kill-on-drop fallback.

Text, thinking, Bash and generic tool activity, approvals, Agent/Task subagents and
result usage become SQLite records/events. Task notifications gate terminal completion;
a tool result acknowledging an agent is insufficient. Native thread/item/request IDs and
parent tool relationships are retained. Native turn identity is weak (app run ID).
Usage is exposed in provider-turn tokenUsage; raw result accounting is a separate bounded
SQLite record. No CLI transcript is retained in RAM. Frames, assistant bodies, reasoning
and subagent progress each have a hard 1 MiB bound; oversized data fails the run.
Channels remain bounded (16 output, 8 control); permission/task maps cap at 64.

`queue_after_active` persists queued runs and starts the next on completion/interruption.
`steer_active` sends an additional user frame into the live process and retains the app run.
Observed native steering consumed input before a single final result. Permission responses
reserve channel capacity before committing so a full channel cannot silently discard them.
Allow, decline and session suggestions map to native control responses. `acceptAlways` is
explicitly rejected for native requests; persistent permission writes are not implemented.

Config advertises installed/missing status, unknown auth, warning readiness and CLI aliases
Haiku/Sonnet/Opus. It does not claim authentication has been checked. No auth/setup UI,
MCP registration bridge, structured question UI, attachment conversion, crash recovery,
queue editing/promotion/restart or cross-provider handoff is implemented. CLI stderr is
not parsed or retained; stdout errors and exit failures surface as run errors. Child-task
notifications are tested from foreground execution; detached/background lifetime behavior
across process restarts is not proven. Full native turn/message/attempt identity, file diffs,
plans and provider session idle/error lifecycle need further work. The child RSS is excluded;
the under-50-MB server target has not been measured with this adapter.

Fixtures and exact probe prompts/flags are documented in `tests/fixtures/claude/README.md`.
Unit replay covers reasoning/tools/subagents/usage/approval projections, failure boundaries,
and oversized/malformed input. Real Connection tests cover queue/steer idempotency, responses,
interrupt and idle mode/model updates. One ignored live test was explicitly run successfully:
Write approval, same-PID second turn, native resume after terminating the process.
Use a private target directory, with debug disabled on quota-constrained machines:

```sh
CARGO_TARGET_DIR=/tmp/o3-claude-target CARGO_PROFILE_DEV_DEBUG=0 \
CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test
# Opt-in: consumes existing Claude credentials and Haiku quota, temporary workspace only.
CARGO_TARGET_DIR=/tmp/o3-claude-target CARGO_PROFILE_DEV_DEBUG=0 \
CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test live_persistent -- --ignored
```

Final verification: 30 default tests passed (one live test ignored by default), plus the
explicit live test passed. Clippy with all targets and warnings denied, formatting and
compat generation passed. A shared disk quota blocked the client's unused staticlib
archive: the final test run used a temporary copy of the identical lib-t3-rust sources
with only `crate-type = ["rlib"]`; the repository manifest was restored afterwards.
