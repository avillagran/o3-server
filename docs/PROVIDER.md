# Provider boundary

`src/provider.rs` owns the `Provider: Send + Sync + 'static` interface. Implement `execute` as a
boxed `Send` future; install an `Arc<dyn Provider>` in `State.provider` before serving requests.
A routing implementation can select a Claude or Codex adapter from `RunInput.model_selection`.
The executable deliberately installs no provider. Production sends fail explicitly rather than
manufacturing assistant output. Tests inject `ScriptedProvider`.

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

## Adapter work still required

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
