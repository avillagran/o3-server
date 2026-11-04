# MCP interoperability

`/mcp` implements Streamable HTTP transport with durable readers and thread control using MCP protocol
`2025-06-18`. This is an incremental port, not full MCP or nightly parity.

## Credentials

Create a dedicated credential against the same profile as the server:

```sh
o3-server auth mcp issue --access read-only --label 'My MCP client'
```

The command prints a secret token; pass it privately as `Authorization: Bearer …`
to the MCP client. Do not include it in logs or commit it. `--token-only` is available
for process-to-process delivery. Default lifetime is 30 days; `--ttl` uses the same
syntax as ordinary session issuance. Existing session list/revoke commands manage
these credentials too. `--help` does not create a profile or issue credentials.

MCP credentials have the `mcp-client` subject and an independent audience. Ordinary
owner/pairing bearer sessions do not authenticate `/mcp`. MCP credentials do not
authenticate the environment's ordinary HTTP, WebSocket or asset routes, including
WebSocket tickets. Expiry and revocation are checked from SQLite on every request.
Proof-bound credentials are rejected until DPoP is implemented, with no bearer
fallback. The auth row's scopes and runtime ceiling are authoritative.

`approval-required`, `auto-accept-edits`, `auto` and `full-access` issuance preserve
upstream MCP scope sets and the runtime ceiling. Read-only credentials cannot mutate.
Thread mutations require orchestration:operate and a target whose current runtime
mode is no broader than the credential's persisted ceiling. Mode order is
approval-required < auto-accept-edits < auto < full-access; execution support remains
provider-specific. External clients normally have the default interaction-mode ceiling. A per-request
T3-Mode-Limit header (`<runtimeMode>/<interactionMode>`) can only narrow it: the runtime
ceiling is the minimum of the grant and request, and plan cannot write a default-mode
target. Asking for a broader header never raises the persisted grant. Malformed headers
are rejected with invalid_mode_limit/400, even before initialization.

Both command dispatchers check current credential expiry, revocation, scopes and the
target's modes under the same SQLite write transaction as the mutation, before
receipt replay. A mode change or credential downgrade while waiting cannot bypass
admission. Authority travels separately from wire commands: callers cannot inject
it or raise their ceiling through arguments. Thread configure changes only the model
selection, with legacy provider/options normalization; it does not change modes.

## Native Claude/Codex credentials

Supervised normal turns receive a provider-owned credential for the same `/mcp`
endpoint, including when agent browser access is disabled. Text-only generation,
provider discovery and the tools-disabled cold summary stage receive none. Internal
injection keys from persisted model selections are removed before issuing credentials.
Claude receives a private temporary configuration file (0600, parent 0700); Codex
receives configuration over app-server stdin on thread start/resume/fork. Bearer
secrets are absent from argv, durable settings, auth-session rows and projections.

The in-memory registry indexes SHA-256 token hashes and weak process/session owners.
It holds at most 256 native contexts. A cached Claude session retains its context;
Codex's process actor retains up to 256 native-thread contexts. Unchanged sessions
reuse their credential across turns. Shutdown, eviction, idle reap or observed process exit revoke it and remove its transport sessions. Claude checks
cached child liveness when a credential is used; Codex revokes on actor exit. An HTTP
request retains only the authorization scope, and cannot prolong its process owner.
Inactivity expires credentials after 24 hours; unexpired HTTP requests renew them,
while trusted provider activity can refresh idle credentials. Revocation is permanent.

Native authority is the calling thread's current modes, reloaded under the mutation
transaction. Native callers ignore T3-Mode-Limit, so a header cannot replace those
limits. Writes require an unarchived caller owned by the same provider instance and
generation, with a current run or a pending subagent from that provider. Commands
and foreign-provider background work grant no authority. Canonical completed tasks
mask stale compatibility records. Reads may continue after foreground completion;
identity changes and deleted callers are refused. Missing optional thread targets
resolve to the caller; missing project targets in list/search resolve to its project.
Thread list reports currentThreadId.

The existing ten scoped browser tools share this transport when enabled. Each scope
owns only its tabs; browser settings are checked on use. Foreground completion closes
its tabs and profiles while the native orchestration credential can remain alive.
The separate per-run browser HTTP listener has been removed. Full current-nightly
preview toolkit equivalence remains pending.

## Transport

Send a JSON-RPC `initialize` request with clientInfo, capabilities and protocolVersion
using POST, Content-Type application/json and Accept application/json (or the usual
application/json, text/event-stream). The server negotiates 2025-06-18 and returns
Mcp-Session-Id. Include this header on later calls, alongside the bearer credential.
An explicit unsupported Mcp-Protocol-Version is rejected. Sessions are owned by the
credential that initialized them; their IDs grant no access by themselves.

Notifications return an empty 202. Requests support ping, tools/list and tools/call;
DELETE terminates the transport session with 204. JSON-RPC errors distinguish malformed
JSON, invalid envelopes/arguments, missing methods and tool execution failures. Tool
results contain both text JSON and matching structuredContent, with isError for failures.

There is no standalone GET SSE stream or resumable event delivery yet: GET returns
405, and an SSE-only Accept header returns 406. Requests with Origin are rejected.
There is no OAuth metadata, browser approval or dynamic registration yet. Native
process-owned credentials use the lifecycle described above; external OAuth and
linked-environment credentials remain pending.

Limits: 1 MiB incoming JSON, 10-second body read, eight simultaneous requests, eight
transport sessions per credential and 128 total. Inactive transport sessions expire
after 30 minutes, and all transport sessions disappear on restart. External MCP bearer sessions
remain persisted and revocable across restarts. Readers run on blocking workers rather
than on async I/O workers.

## Published readers

- t3_environment_read: identity/platform and selected preferences; custom writing
  instructions are limited to 4,000 Unicode characters.
- t3_project_list / t3_project_read: live project pages and individual project settings.
- t3_thread_configuration: model selection and modes; an external client passes threadId.
- t3_thread_list: newest-updated-first project pages, title/status/settled/snoozed filters,
  lineage, modes and PR links; an external client passes projectId.
- t3_thread_search: existing bounded title/content search with optional project filters.
- t3_thread_read: durable detail, recent runs, message/activity timeline pages and text
  continuation using itemId/textOffset. Offsets count UTF-16 code units. A page boundary
  inside an astral character returns the complete character and advances past it,
  avoiding lost text; this may exceed the requested text limit by one UTF-16 unit
  and differs from upstream JavaScript slicing of lone surrogates. At most 100 items,
  50 recent runs, 50,000 text units per item and roughly 2 MiB of timeline items per page;
  the first item is retained even if it alone exceeds the page budget. A shortened page
  reports hasMore and resumes after nextPosition. Item bodies are read one at a time.

References: T3 Code's MIT source at `680152e27b2a961686389ca69a1557cbb2c55f08`,
`apps/server/src/mcp/McpHttpServer.ts`, toolkit tools/handlers, McpInvocationContext,
McpToolAccess, OrchestratorMcpService, auth/EnvironmentAuth and packages/contracts.
The monitored 4119 source has the same transport protocol and credential separation;
full behavioral equivalence to that nightly has not been established. The upstream
MIT notice is retained in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).


## Thread control

The thread-control port introduced 23 core tools, retaining the earlier granular queue/question names
alongside nightly 4151's grouped t3_queue and t3_pending_request. Their action selects
the same read or write path: read-only credentials can list/read, and cannot edit,
cancel, promote or respond. External clients pass threadId explicitly.

All tools accept environmentId. The current environment is allowed; a foreign target
returns a capability failure before local execution, until linked routing is ported.
Thread list supports allProjects=true (projectId=null in the result), includes projectId
on each item and treats statuses=[] as no filter. Thread read clamps positive limit,
runLimit and maxCharsPerItem above their documented maxima, matching the current schema;
zero remains invalid. Configure without modelSelection reads the current configuration.

- t3_thread_send: send plain text with auto, queue or steer delivery. Auto uses the
  existing capability-aware steering/queue behavior; explicit steer refuses a thread
  with no steerable live turn instead of silently queuing. Restart delivery remains
  unimplemented and returns an explicit invalid_request failure.
- t3_thread_interrupt: interrupt a named or active run, report already-terminal state
  or no_active_run. This does not hold or cancel queued messages, as in upstream's
  interrupt operation; queue cancellation is separate.
- t3_thread_wait: await the selected run's durable terminal state, with timedOut for
  unfinished work. Omitted runId pins the latest run at call time; idle returns
  immediately. Default wait is ten minutes, capped at one hour. It awaits notifications
  outside blocking workers and rechecks credential liveness at least once per second.
  Restart-continuation following remains pending with restart support.
- t3_thread_organize: pin, unpin, snooze, unsnooze, settle, unsettle, archive, unarchive,
  or mark unread through the normal lifecycle. Caller-thread deferred settlement
  remains unimplemented.
- t3_thread_configure: read configuration, or change provider/model selection without changing permission modes.
- t3_queue_list / t3_queue_read / t3_queue_edit / t3_queue_cancel / t3_queue_reorder /
  t3_queue_promote_to_steer: operate on the real queue using its existing ordering,
  text limits, attachment preservation, status validation and native control path.
- t3_pending_request_list / t3_pending_request_read / t3_pending_request_respond:
  expose pending user questions only. Response admission checks the request's actual
  stored thread and kind, so this cannot approve a permission request.

Send and interrupt accept clientRequestId. IDs are namespaced by the authenticated
credential, operation and key, independently of the transport session. Receipts are
committed with the underlying effect before native delivery. Retrying a send reuses
its message/run IDs and original delivery classification; status reflects the current
run. Interrupt retries report terminal state once available. MCP messages record
createdBy=agent and creationSource=mcp. Read/write results share the ordinary SQLite
projection, event log and control outbox.

## Evidence and remaining work

`tests/mcp.rs` exercises real loopback HTTP and the real Connection's project/thread
mutations. Tests failed before this port for the missing endpoint and MCP credentials
escaping into ordinary HTTP. Positive cases cover listing/reading real state, transcript
pagination/text continuation, Unicode, matching structured/text results, and CLI issuance.
Negative cases cover foreign transport sessions, deleted sessions, revocation, body limits,
invalid JSON/schema/envelopes, unsupported methods/tools, Origin and ordinary session
rejection. These are deterministic protocol tests, not a live nightly differential or
acceptance of the existing ChatGPT/OmaBridge integration.

Pending: OAuth/DPoP, full SSE semantics,
thread launch/restart/transfer, worktree handoff, deferred caller settlement, task delegation,
remaining worktree/attachment/PR/device/preview/HTML/scheduler toolkits and linked environments.
Inherited-task acknowledgement and exact activity-text rendering also need upstream
parity. No daily profile, external MCP integration or running service was changed.

The initial reader port passed full suites 307 default / 304 no-default, with 21
opt-ins ignored in each. Thread-control regressions additionally cover read-only and
runtime refusal, target/credential changes, receipt deduplication, agent provenance,
queue lifecycle, native steering/promotion, interrupt/wait and a user-question round
trip through native projection. A stale-authority unit regression covers both write
transactions and confirms denial before receipt replay. The public mutation regression
fails against the previous reader-only commit. Native providers here are deterministic
fixtures; no real model inference or live nightly differential is claimed.

Nightly intake was refreshed on 2026-11-04 to 4151,
`322f6544dfad0a3c8a0c8cbecf2c7f7874b14268`. Contract/source comparison added the
grouped tools, configure read overload, list item projectId/allProjects, empty statuses,
read-limit clamping and explicit foreign-environment refusal above. See
[nightly/mcp-thread-control-intake.json](nightly/mcp-thread-control-intake.json) for the
release references and remaining changes; this does not update the validated 3726
RPC baseline or certify complete 4151 equivalence. Hosted links and linked routing remain pending with their enabling features. Native
caller policy and automatic Claude/Codex injection are implemented as described above. The per-request T3-Mode-Limit narrowing itself is implemented.

Final validation after the 4151 alignment: 316 default / 313 no-default tests pass,
with 21 opt-ins ignored each; 16 HTTP MCP cases and the transaction-authority unit
regression are included. Strict Clippy passes in both modes; fmt, generated
compatibility and all nine nightly-monitor tests pass. No live deployment, provider
inference or complete-nightly conformance is asserted.

## Native credential validation (2026-11-04)

The native credential work uses nightly 4151's McpSessionRegistry, McpInvocationContext
and threadAccess at `322f6544dfad0a3c8a0c8cbecf2c7f7874b14268`
([intake](nightly/native-mcp-intake.json)). Three registry/policy
unit regressions cover expiry, trusted refresh, request references surviving owner
closure, current mode changes, background ownership and generation replacement.
Subprocess Claude/Codex peers perform real HTTP MCP initialization and calls; four
turns exercise reuse, browser disable, abrupt process death, fresh credentials and
shutdown. They assert configuration file permissions, argv isolation, missing-target
defaults, lower-mode rejection, inactive-parent rejection and ordinary HTTP/WS denial.
These deterministic peers are not a live inference or compatible desktop acceptance test.

Final native-credential validation: 319 default / 316 no-default tests pass, with
21 opt-ins ignored in each mode. Strict Clippy passes in both modes; fmt, generated
compatibility and all nine nightly-monitor tests pass. No running-service deployment
or complete-nightly conformance is asserted.

## Project mutations and worktree readers (2026-11-04)

Six tools extend the core catalog to 29 (plus the ten existing native browser tools
when enabled). `t3_project_create`, `t3_project_update`, `t3_project_delete` and
`t3_project_clone` require a live full-access/default caller or a full-access client.
Mode-limit narrowing remains authoritative. Project record mutations recheck the
credential and caller under the same transaction, before receipt replay. Workspace
uniqueness and nonempty deletion are checked there too.

Create registers an existing folder, or creates it with createWorkspaceRootIfMissing.
Omitting workspaceRoot uses the shared named-project scaffold (git, README, icon and
initial commit). A commit failure is returned as commitError while keeping the project.
A title-only creation rejects scripts/defaults; set them afterwards. As in the source
ProjectService, creation does not record defaultModelSelection; update does. Update
preserves omitted fields, supports nullable defaults/icon/favicon, scripts, pin/archive
and autoPull. Monograms allow at most two grapheme clusters. New script IDs must follow
the source's 1–24 lowercase letter/digit/hyphen rule; existing invalid IDs may be edited.
Delete refuses a nonempty project without force and retains its folder. Forced project
deletion uses the same receipt-atomic thread-deletion plan as ordinary thread.delete:
child tombstones, cancelled work and cleanup jobs commit together. The durable resource
lifecycle is described below.

Clone creates an independent checkout: it does not register or adopt an existing project.
The same implementation now handles ordinary sourceControl.cloneRepository. It accepts
raw clone URLs/local paths and GitHub repository lookup through gh. Auto selects SSH for
lookup and retries authentication/host-key rejection over HTTPS; explicit ssh never
falls back. Git arguments use argv and --; destinations are serialized and must be new
or empty. Failed calls never delete occupied destinations. Returned URLs redact URL
credentials, and errors do not expose Git stderr. Other hosts can use remoteUrl; their
repository lookup adapters remain pending.

`t3_worktree_list` reads the selected thread's project/worktree with the existing ref
pagination and filters, including read-only clients. Native callers may omit threadId.
If cleanup removed an attached path, listing falls back to the project checkout.
`t3_worktree_status` needs a native thread credential and returns attached/path/branch,
project root and the effective per-project startFromOrigin default. Source reference:
nightly4151 project/worktree toolkits, ProjectService/ProjectCommands, ManagedProjectFolders,
SourceControlRepositoryService and WorktreeMcpService
([intake](nightly/mcp-projects-intake.json)). This port does not yet implement
handoff: expected-path comparison, provider detachment, setup and queued continuation
must be ported together. Thread launch remains pending too.

Tests use actual MCP HTTP and the ordinary client to inspect durable CRUD and settings,
real Git checkouts/clones, rejected permissions with no filesystem effects, malformed
schemas, native defaults and cleanup fallback. A separate subprocess fixture exercises
gh lookup, SSH/HTTPS selection and fallback, redacted errors and initial-commit failure.
A stale-authority unit regression bypasses preflight to verify transaction admission and
receipt replay. These checks do not establish live host/network or desktop acceptance.

Validation: full suites pass with 326 default / 323 no-default tests (21 opt-ins
ignored each). Final MCP/project/clone/native regressions and strict Clippy pass in
both configurations; fmt, generated compatibility and nine nightly-monitor tests pass.
The existing compatible desktop and live hosted-network flows were not exercised.


## Durable thread and project deletion (2026-11-04)

Ordinary thread deletion and forced project deletion now share a transaction-scoped
planner, adapted from nightly4151 ThreadDeletion, ResourceCleanupService and EffectWorker.
It cancels active runs, attempts, nodes, provider turns, background tasks and pending
requests; detaches provider-session projections and disposes app-owned completion
cohorts without reopening terminal runs. Thread tombstones, child events, cleanup jobs,
attachment references and the command receipt commit atomically. Project authority is
still checked before receipt replay. Startup plans missing cleanup for legacy tombstones
before ordinary recovery can resume their orphaned work.

After commit, native credentials are revoked and supervised executions are cancelled.
A persistent outbox retries disposal after failure or restart. It stops cached Claude
sessions and unloads only the affected Codex native threads with thread/unsubscribe,
including sessions in retired provider pools and late native binding replies. Other
threads sharing a Codex app-server keep their process, credentials and pending work.
A transient unsubscribe failure remains retryable until the provider acknowledges it.

Disposal closes terminals and removes their stored history, closes previews, deletes
thread-owned scheduled tasks, finishes command-output tails and removes attachment files
plus text sidecars. Shared attachments survive while another live thread references them.
Retries also remove a sidecar left behind after its main file was removed. Project folders
and worktrees are retained. Deleted threads reject terminal/preview reopening and late
provider/command-output persistence.

Four deletion tests use real HTTP MCP and subprocess Claude/Codex protocol peers. They
cover self-project deletion during a pending approval, unaffected shared-process siblings,
provider switches, transient native unload errors, a delayed native binding reply,
transaction rollback/replay, restart recovery, legacy tombstones, shared assets and a
filesystem cleanup failure. Terminal processes, persisted history, preview groups, task
records and files are inspected. The peers do not establish installed-provider inference
or compatible desktop acceptance.

Validation for the combined project/deletion block: full suites pass with 330 default /
327 no-default tests (21 opt-ins ignored each); final Codex and deletion/provider/MCP/
background/browser regressions and strict Clippy pass in both configurations. fmt,
generated compatibility and nine nightly-monitor tests pass. Latest source rechecked at
06:06 UTC: nightly4151, 322f6544dfad0a3c8a0c8cbecf2c7f7874b14268.
Worktree handoff, launch and the remaining protocol backlog stay open.


## Workspace transition prerequisite (2026-11-04)

The ordinary thread.metadata.update command now checks expectedWorktreePath,
expectedBranch and expectedEmpty inside its receipt transaction. Explicit null is a
comparison value. Title-only edits and unchanged worktree paths leave native sessions
attached. A real path change interrupts live work, cancels pending native requests and
records provider-session detachment with a persistent effect, while preserving queued
messages, native conversation IDs, files and terminal resources.

The effect waits for supervised runs to stop, unloads cached native sessions and then
releases queued work in the new directory. Failures retry; startup continues pending
jobs. Native-detach acknowledgement and queue release have separate durable markers,
so a restart can release the queue without unloading a successor's session. Immediate
starts and steering cannot cross a pending detach barrier. Cached Claude processes are
replaced with --resume; Codex unloads only the affected native thread, then resumes its
identity with the new cwd. A live shared Codex process retains the same MCP credential
across this ordinary detach, as in ProviderSessionManager; deletion still revokes it.
Late binding replies are unloaded unless a live successor owns or is resuming that ID.

This supplies the shared transition needed by t3_worktree_handoff. That MCP tool still
requires branch/base selection, fetch/create/rollback, setup and durable continuation
orchestration; it is not advertised yet. Thread launch and the wider protocol backlog
remain pending. Deterministic native peers verify directory/identity, credentials,
sibling isolation, retry and delayed resume; module tests cover transaction rollback,
comparison races, no-ops and two restart boundaries. Installed-provider inference and
compatible desktop acceptance were not exercised.

Validation for this block: full suites pass with 333 default / 330 no-default tests,
21 opt-ins ignored each. Final transaction/recovery module tests plus 32 workspace,
deletion, queue/thread-command and HTTP MCP regressions pass per configuration.
Strict Clippy passes in both modes; fmt, generated compatibility and all nine
nightly-monitor tests pass. No service deployment occurred.
