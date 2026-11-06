# Conformance evidence

## Terminal resource scopes, 2026-11-06

Interactive terminals and managed project/setup PTYs now use the nightly general
collecting agents slice rather than the per-profile orphan slice. The registry
constructs one shared scope command policy for native providers and terminals.
Terminal launches keep their explicit sanitized environment; an empty/missing
runtime directory falls back to the server runtime. Resource discovery is awaited
only for an actual spawn, before taking synchronous PTY locks. The owned setup
subreaper accepts wrapper argv, with all CString storage prepared before fork,
and adjusts the executed child OOM score using only async-signal-safe syscalls.

The complete container suites pass **533 default / 530 minimal**, no failures,
36 opt-in cases ignored per configuration. Strict Clippy, formatting, compatibility
generation and nine nightly-watch checks pass. New checks cover manager-free
PTY fallback without parent-environment inheritance and invalid open/attach
requests without policy initialization. Upstream Terminal Manager source is
unchanged between4262 and4275.

The real systemd opt-in case passes in both modes. Interactive and managed Python
PTYs preserve PID=session=process-group identity, controlling TTY, a literal
environment value, absence of an excluded parent variable, OOM score800 and the
general agents slice. A terminal API request initializes the backend lazily, and
profile orphan cleanup leaves the PTYs alive. This verifies resource policy and
process ownership; it does not add persistent PTY reconnection after a real server
restart. No live services were deployed.

## Server-browser resource scopes, 2026-11-06

Server browsers now use the profile registry and the upstream unowned collecting
scope policy. Scope preparation precedes CDP descriptor allocation; the duplicate
uses an owned file descriptor on every failure path. The actor waits for scope
cleanup before reporting completion. Human and agent previews reuse the same
launcher and shared browser profiles retain their process until the final tab closes.

Nine affected compatible-client cases pass in each feature mode, along with
strict Clippy. The real systemd opt-in case also passes in both modes: descriptors
3/4 carry CDP commands and screenshot replies through systemd-run, the process
has OOM score800 and belongs to the expected profile slice, closing one tab
preserves a sibling, and final shutdown reaps the browser process. This uses a
private Python CDP peer, not an actual Chromium renderer. Upstream ServerBrowser
source is unchanged between4262 and4275. No host deployment occurred.

## Native agent resource scopes, 2026-11-06

The headless server enables one per-profile Linux registry for native Claude and
Codex processes, including configured instances and auxiliary discovery/quota
actors. Embedded hosts opt in explicitly. The registry mirrors nightly 4275's
systemd scope discovery, owned-slice hash, orphan cleanup, memory headroom,
CPU/IO/task weights, OOMPolicy fallback, scope tracking, delayed OOM diagnosis and
stop/kill guards. Child OOM adjustment uses only async-signal-safe syscalls before
exec; it does not add a shell wrapper. Adapter shutdown awaits scope cleanup.
Codex actors retain shared ownership and cannot be killed for one thread. Joining
an existing actor clears stale ownership from another session; an old lease cannot
clear its replacement. Claude descendants receive the trusted app thread ID.

The six scoped upstream files are unchanged between 4262 and 4275. Executing the
actual upstream helpers produced **163 cases** for profile hashes, memory limits
and scope classification. Fourteen container checks cover lifecycle/races,
fallbacks, configured Claude/Codex transports and the real compatible client's
confirmed OOM message. The complete suites passed **531 default / 528 minimal**,
no failures, 35 opt-in tests ignored per mode, before adding the dedicated native
systemd opt-in case.

`tools/test-agent-systemd` compiles that additional case and runs it in a disposable
rootless container with a real systemd user manager, private PID/cgroup namespaces,
a read-only root and binary mount, no network or host bus, and a 768 MiB outer
memory limit. It verifies detached `setsid` descendants are reaped after parent
exit, `cgroup.kill` terminates the owned tree, shared processes survive a per-thread
kill request, an asserted 32 MiB kernel memory cap produces a sticky OOM result,
and restarted-profile cleanup preserves another profile. The container is removed
after success or failure. The initial OOM fixture timed out with a separate 24 MiB
throttling limit; the final fixture disables its own throttling and checks the
kernel hard cap before allocation. Production headroom limits remain unchanged.

The native case and all fourteen affected checks passed in both feature modes;
strict Clippy also passed in both modes. This is resource/lifecycle acceptance, not an
installed authenticated inference comparison. Public `forceStopThread`, PTY and
browser scope integration, and complete typed failure cards (including `oom_kill`)
remain open; the run currently exposes the confirmed OOM message. No live host
was deployed or had its service manager altered. See
[`nightly/agent-scope-intake.json`](nightly/agent-scope-intake.json).

## Rust test isolation, 2026-11-06

`tools/test-rust-sandbox` compiles with Cargo on the host, then executes every
native test binary in rootless Podman. The source checkout, Git metadata, test
binaries and private Node installation are read-only mounts; the container root
is read-only, external networking is disabled and all fixture writes stay in
private temporary filesystems. The runner checks mount flags before executing
tests, forwards Cargo name filters, limits memory/processes and stops its own
container on completion or interruption. The local derived image adds ffmpeg to
`localhost/android-sandbox:latest`; it is not published.

Both complete configurations passed: **517 default / 514 minimal**, no failures,
35 opt-in tests ignored per configuration, 95 executable groups each. The
managed-provider fixture still exercises installation through its private npm
peer; Node comes from a read-only runtime mount. A process-parent death regression
also passed independently in the container. The runner does not supply real
credentials, hardware, an official nightly or network access to ignored tests.

Test portability fixes use the configured temporary directory and let a pure
process-parent fixture run without Node. The differential harness no longer
recursively removes old profiles by matching broad prefixes in a shared host
directory; its parent-death fixture removes only its own pid file and empty
directory. CI also executes Rust checks inside a disposable job container,
using the container configuration already validated by the parallel fresh-context
branch. This block changes verification infrastructure, not nightly feature
coverage or any deployed host.

This is a partial operational checkpoint, not a full interchangeability certificate.
The owner reprioritized startup memory and P0 operation; unresolved work is listed below.

Reproduce with `tools/conformance-diff` (nice, cargo -j 4, no bridge feature). It starts
one fresh T3 0.1.7-nightly.20261031.3991 and one fresh o3 on independent free loopback
ports under `/var/tmp/o3-build/tmp/differential`. `T3_REFERENCE_BIN` can select another binary.
`HOME`, XDG roots, provider environment and `T3CODE_HOME` are isolated. Node's actual
runtime path is used instead of a HOME-sensitive mise shim. The user's running T3,
profile and service are never used. Servers have private process groups; normal exit
terminates them and removes profile/workspace state. Reports retain shapes only.
No inference credentials are imported. Builds use `/var/tmp/o3-build/main`; TMPDIR is `/var/tmp/o3-build/tmp`.

`O3_DIFF_STRICT=1` fails on any shape difference. The default is a discovery run:
its zero exit status means the probes ran, not that equivalence passed. The committed
`conformance-report.json` is the sanitized evidence from provider commit 52efa96
plus the Claude idle-reaping/resolver tests. The final resolver bin-directory fix
was tested separately; it changes executable selection, not this wire report.

Shapes retain field names, scalar types, absent-vs-null, array length/order, literal
union/error tags, HTTP/CLI status codes and semantic provider identifiers. Other ids,
timestamps, paths, URLs and scalar values are erased. RPCs, uploads, pairing and
subscriptions use the pinned real t3-protocol Connection; failed unary calls are also
inspected as raw Effect frames so client error-string normalization cannot hide tags.
Raw failure retries only use rejected fixture operations. Positive command receipts
are not retried by the raw observer. Stream capture is bounded (first one/two items),
so this does not certify arbitrary replay, interleavings or slow-client behavior.

Three existing client tests were also run against **each** isolated server using
`O3_CONFORMANCE_BASE`/`O3_CONFORMANCE_PAIRING`: one-use pairing + authenticated RPC,
forged credential rejection, and initial shell subscription. Reproduce by first
building `client_compat` and setting `O3_DIFF_CLIENT_TEST_BIN` to that test executable.

## Fixes verified

- CLI sessions now receive standard scopes when --scope is omitted; explicit scopes
  still replace defaults. Previously the CLI token could not read config or shell.
- Dispatch failures include the pinned required commandType plus structured detail/cause.
- Project creation initializes missing default/null metadata.
- Terminal snapshots include current cols/rows; open/attach/write/resize match the sample.
- VCS status always includes zero aheadOfDefaultCount and branchChanges defaults.
- Missing/invalid history cursors return HTTP 400 rather than being mistaken for 404.
- searchThread implements literal message/plan navigation, offsets, wrap and index/start.
  Positive shape matches upstream; rich rendered text/context/skills search remains partial.
- discoverSourceControl probes Git/jj with bounded subprocesses and Effect Options.
  Host-provider discovery is still partial.

## Operational provider fixes

Discovery no longer launches Codex app-server: binary resolution is a filesystem check,
auth remains honestly unknown until execution, model/account queries happen on the first
turn. The isolated executable was observed with **zero startup child processes**.
Codex stops after 60 seconds without pending RPCs/subscribed runs and reconnects lazily;
active runs keep it alive. Cached Claude sessions are killed/reaped after 60 seconds idle,
with generation checks so an old timer cannot kill a newly reused session. Their native
ids remain in SQLite for resume. Mock-actor and child-reaping tests verify the lifetimes;
a new sustained live RSS measurement is not claimed.

T3's real config reports `claudeAgent:claudeAgent`; o3 now advertises that default.
Legacy `claude` and `claude-code` selections still route to Claude. Native projections
preserve the actual selected instance id instead of hardcoding the old alias.
The model picker is intentionally a portable CLI catalog (haiku/sonnet/opus). The
clients consume provider models as slug/name selections, not a fixed enum. Full model
ids/custom selections are passed unchanged to --model; upstream manifest metadata,
context windows and option descriptors are not yet reproduced.

The executable resolver bypasses mise command-wrappers/shims by using the installed
global/latest executable (both bin/tool and tool layouts), then canonicalizes it. It
does not call mise under the isolated HOME. Tests use a wrapper that fails if invoked.
The installed native Codex 0.166.0 app-server --help was checked (supports --stdio).

## Accepted differences and open work

Only the explicit version/optional differences below are accepted. Being observed on
3991 does not by itself establish drift from 3726. Required pinned fields remain bugs.
The pinned source is `~/Work/t3-reference-3726`, contracts at 3f9448747.

- **Version drift:** server.getSettings is unknown on 3991. Keep it for the pinned client.
  Newer config/settings/capability fields are recorded, but need per-field validation
  before acceptance; unsupported capabilities remain absent/off.
- **Optional representation:** extra diagnostic detail/cause in Git errors and omitted
  pinned optional snooze/pull-request fields do not invalidate the pinned decoder.
  The strict report deliberately keeps these differences visible.
- **Open P0:** full shell metadata and live stream parity, worktree failure
  cleanup, native control acknowledgement, attachment steering and active recovery. Local scripted/native-peer coverage is not
  presented as upstream inference proof. The five P0 partial RPCs remain partial.
- **Open P1:** rich thread-find segmentation/folded answers; host source-control adapters;
  settings/provider catalog normalization; title search behavior; CLI auth metadata.
  The two former todo RPCs become partial, not done without the missing behavior.
- Every P0/P1 RPC is sampled. P0/P1 command tags beyond create/deferred-message have
  registry-driven **negative** probes; positive lifecycle/queue behavior relies on local
  real-client tests. The negative probes do not certify all valid command payloads.
- HTTP upload + signed asset GET, OAuth pairing, shell, bounded/history and selected auth
  routes are covered. Auth revocation/connect routes and full history/asset Range/HEAD
  differential coverage are incomplete (o3-only tests cover several of them).
- CLI session issue, list, pairing create, runtime-port, wait-ready and serve are exercised.
  Service install/start/stop/update/cloud commands are deliberately not executed: they
  would violate the request to leave the running service alone. o3 dry-run unit generation
  is locally tested. No P2 expansion or deployment was performed.

## Validation

Before the operational changes: 94 tests passed with --no-default-features; two live
provider tests and the opt-in differential test were ignored. After provider changes:
57 selected tests passed (lib, CLI, client_compat, codex_contract, thread_commands),
including lazy startup, idle restart/reaping, alias compatibility through existing tests
and shim resolution. Strict all-target clippy, format and diff checks passed with -j 4.
All isolated differential servers were shut down after their run. No push was performed.

## Per-surface results

121 probes: 96 matching shapes; 25 differences. A matching rejection is explicitly
labelled and does not promote a compatibility row to done.

| Surface | Result | Classification / limit |
|---|---|---|
| `CLI __ssh-helper runtime-port` | match | Matching sampled shape/order/status; bounded fixture only. |
| `CLI __ssh-helper wait-ready` | match | Matching sampled shape/order/status; bounded fixture only. |
| `CLI auth pairing create` | different | Open CLI JSON metadata difference; helpers and scoped token issuance work. |
| `CLI auth session list` | different | Open CLI JSON metadata difference; helpers and scoped token issuance work. |
| `GET /.well-known/t3/environment` | different | Open config/catalog difference; newer capability/settings additions partly drift, not blanket-accepted. |
| `GET /api/auth/clients` | different | Open auth metadata/scope-management differences; bearer pairing and session read work. |
| `GET /api/auth/pairing-links` | different | Open auth metadata/scope-management differences; bearer pairing and session read work. |
| `GET /api/auth/session` | different | Open auth metadata/scope-management differences; bearer pairing and session read work. |
| `GET /api/orchestration/shell` | match | Matching sampled shape/order/status; bounded fixture only. |
| `GET /api/orchestration/threads/t1/bounded` | different | Open projection shape difference; includes optional snooze/PR fields. Empty launch succeeds on both. |
| `GET /api/orchestration/threads/t1/history` | match | Matching sampled shape/order/status; bounded fixture only. |
| `GET /api/projects` | match | Matching sampled shape/order/status; bounded fixture only. |
| `O3_CONFORMANCE_BASE pairing_is_one_use_and_session_authenticates_rpc` | match | Matching sampled shape/order/status; bounded fixture only. |
| `O3_CONFORMANCE_BASE rpc_requires_a_valid_credential` | match | Matching sampled shape/order/status; bounded fixture only. |
| `O3_CONFORMANCE_BASE shell_subscription_opens_with_a_snapshot` | match | Matching sampled shape/order/status; bounded fixture only. |
| `POST /api/attachments/upload real client` | match | Matching sampled shape/order/status; bounded fixture only. |
| `POST /api/auth/pairing-token` | match | Matching sampled shape/order/status; bounded fixture only. |
| `POST /oauth/token real pairing` | match | Matching sampled shape/order/status; bounded fixture only. |
| `assets.createUrl` | match | Matching sampled shape/order/status; bounded fixture only. |
| `assets.createUrl attachment` | match | Matching sampled shape/order/status; bounded fixture only. |
| `attachments.createUploadUrl` | match | Matching sampled shape/order/status; bounded fixture only. |
| `command-frame queue.resume` | match | Matching rejection shape; not successful feature proof. |
| `command-frame queued-message.promote-to-steer` | match | Matching rejection shape; not successful feature proof. |
| `command-frame queued-run.cancel` | match | Matching rejection shape; not successful feature proof. |
| `command-frame queued-run.edit` | match | Matching rejection shape; not successful feature proof. |
| `command-frame queued-run.reorder` | match | Matching rejection shape; not successful feature proof. |
| `command-frame run.interrupt` | match | Matching rejection shape; not successful feature proof. |
| `command-frame runtime-request.respond` | different | Open validation/schema difference in missing-thread/request probe; no native approval proof. |
| `command-frame thread.active.reorder` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.archive` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.auto-settle.set` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.delete` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.fork` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.interaction-mode.set` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.mark-unread` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.metadata.update` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.model-selection.set` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.pin` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.pin.reorder` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.runtime-mode.set` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.settle` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.snooze` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.unarchive` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.unpin` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.unsettle` | different | Open validation/schema difference in missing-thread/request probe; no native approval proof. |
| `command-frame thread.unsnooze` | different | Open validation/schema difference in missing-thread/request probe; no native approval proof. |
| `command-frame thread.user-input.dismiss` | match | Matching rejection shape; not successful feature proof. |
| `command-frame thread.visit` | different | Open validation/schema difference in missing-thread/request probe; no native approval proof. |
| `command-negative queue.resume` | match | Matching rejection shape; not successful feature proof. |
| `command-negative queued-message.promote-to-steer` | match | Matching rejection shape; not successful feature proof. |
| `command-negative queued-run.cancel` | match | Matching rejection shape; not successful feature proof. |
| `command-negative queued-run.edit` | match | Matching rejection shape; not successful feature proof. |
| `command-negative queued-run.reorder` | match | Matching rejection shape; not successful feature proof. |
| `command-negative run.interrupt` | match | Matching rejection shape; not successful feature proof. |
| `command-negative runtime-request.respond` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.active.reorder` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.archive` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.auto-settle.set` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.delete` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.fork` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.interaction-mode.set` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.mark-unread` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.metadata.update` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.model-selection.set` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.pin` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.pin.reorder` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.runtime-mode.set` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.settle` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.snooze` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.unarchive` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.unpin` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.unsettle` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.unsnooze` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.user-input.dismiss` | match | Matching rejection shape; not successful feature proof. |
| `command-negative thread.visit` | match | Matching rejection shape; not successful feature proof. |
| `error-frame secrets.answerRequest` | match | Matching rejection shape; not successful feature proof. |
| `error-frame server.getSettings` | different | Version drift: 3991 removed tag; pinned 3726 endpoint retained. |
| `error-frame vcs.pull` | different | Accepted optional diagnostic metadata difference (GitCommandError); tag matches. |
| `filesystem.browse` | match | Matching sampled shape/order/status; bounded fixture only. |
| `git.runStackedAction` | match | Matching sampled shape/order/status; bounded fixture only. |
| `orchestration.dispatchCommand` | match | Matching sampled shape/order/status; bounded fixture only. |
| `orchestration.dispatchCommand:message.dispatch` | match | Matching sampled shape/order/status; bounded fixture only. |
| `orchestration.getFullThreadDiff` | match | Matching sampled shape/order/status; bounded fixture only. |
| `orchestration.getTurnDiff` | match | Matching sampled shape/order/status; bounded fixture only. |
| `orchestration.getTurnItem` | match | Matching sampled shape/order/status; bounded fixture only. |
| `orchestration.launchThread` | different | Open projection shape difference; includes optional snooze/PR fields. Empty launch succeeds on both. |
| `orchestration.searchThread` | match | Matching sampled shape/order/status; bounded fixture only. |
| `orchestration.searchThreads` | different | Open behavior difference: title-only match and snapshotSequence extension; not certified. |
| `orchestration.subscribeCommandOutput` | match | Matching sampled shape/order/status; bounded fixture only. |
| `orchestration.subscribeShell` | different | Open metadata/stream ordering difference; initial client subscription works. |
| `orchestration.subscribeThread` | different | Open metadata/stream ordering difference; initial client subscription works. |
| `projects.mutate` | match | Matching sampled shape/order/status; bounded fixture only. |
| `projects.readFile` | match | Matching sampled shape/order/status; bounded fixture only. |
| `projects.searchEntries` | match | Matching sampled shape/order/status; bounded fixture only. |
| `provider identities and model slugs` | different | Open config/catalog difference; newer capability/settings additions partly drift, not blanket-accepted. |
| `review.getDiffFileContents` | match | Matching sampled shape/order/status; bounded fixture only. |
| `review.getDiffPreview` | match | Matching sampled shape/order/status; bounded fixture only. |
| `secrets.answerRequest` | match | Matching rejection shape; not successful feature proof. |
| `server.discoverSourceControl` | different | Open host adapter coverage: Git/jj probes work; GitHub/GitLab descriptors/auth not implemented. |
| `server.getConfig` | different | Open config/catalog difference; newer capability/settings additions partly drift, not blanket-accepted. |
| `server.getSettings` | different | Version drift: 3991 removed tag; pinned 3726 endpoint retained. |
| `server.refreshProviders` | different | Open config/catalog difference; newer capability/settings additions partly drift, not blanket-accepted. |
| `server.reportClientActivity` | match | Matching sampled shape/order/status; bounded fixture only. |
| `server.updateSettings` | different | Open config/catalog difference; newer capability/settings additions partly drift, not blanket-accepted. |
| `status GET /.well-known/t3/environment protocol=false auth=false` | match | Matching sampled shape/order/status; bounded fixture only. |
| `status GET /api/assets/forged/file.txt protocol=false auth=false` | match | Matching sampled shape/order/status; bounded fixture only. |
| `status GET /api/auth/session protocol=false auth=false` | match | Matching sampled shape/order/status; bounded fixture only. |
| `status GET /api/orchestration/shell protocol=false auth=true` | different | Pinned policy retained: o3 requires protocol 2 header (426); 3991 accepts omission. |
| `status GET /api/orchestration/shell protocol=true auth=true` | match | Matching sampled shape/order/status; bounded fixture only. |
| `status GET /ws?orchestrationProtocol=1 protocol=false auth=true` | match | Matching sampled shape/order/status; bounded fixture only. |
| `status GET signed attachment` | match | Matching sampled shape/order/status; bounded fixture only. |
| `subscribeServerConfig` | different | Open config/catalog difference; newer capability/settings additions partly drift, not blanket-accepted. |
| `terminal.attach` | match | Matching sampled shape/order/status; bounded fixture only. |
| `terminal.open` | match | Matching sampled shape/order/status; bounded fixture only. |
| `terminal.resize` | match | Matching sampled shape/order/status; bounded fixture only. |
| `terminal.write` | match | Matching sampled shape/order/status; bounded fixture only. |
| `vcs.createRef` | match | Matching sampled shape/order/status; bounded fixture only. |
| `vcs.listRefs` | match | Matching sampled shape/order/status; bounded fixture only. |
| `vcs.pull` | match | Matching rejection shape; not successful feature proof. |
| `vcs.refreshStatus` | match | Matching sampled shape/order/status; bounded fixture only. |
| `vcs.switchRef` | match | Matching sampled shape/order/status; bounded fixture only. |

## Current P0 implementation checkpoint

The five P0 method rows remain partial, with explicit current differences in
compat-status.json / COMPATIBILITY.md: multi-commit launch/workspace side effects;
shell metadata/repository replacement snapshots; thread field/order/attempt metadata;
native chat attachments/policies/questions/settlement; config/provider catalog metadata.
This checkpoint does not rerun the upstream differential or change its evidence.

New local proof: 101 tests pass (3 ignored), including durable restart IDs for both
adapters and receipt/outbox rollback/reopen. Native desktop embedded management,
pairing/approval and control-socket T3 pair/forward pass (E2E.md). Durable handoff
journaling does not certify exactly-once native delivery; see RECOVERY.md.

## Daily-chat checkpoint, 2026-11-01

`tests/provider_input.rs` uses the pinned real Connection, actual upload HTTP calls,
and deterministic native CLI peers. It verifies image/file/context delivery to both
adapters, injected launch rollback and retry, and Claude structured questions via
`t3_protocol::questions`. Claude's catalog comes from the local pinned 3726 manifest;
option compilation is tested, with actual CLI arguments checked by the peer.
Shell user/run timestamps used by desktop sidebar now reflect stored records.

`tools/audit-client-fields` inventories differing P0 field names from the retained
3991 report across shared library, desktop and Android source (including Kotlin).
This is an inventory, not a fresh differential run or proof that a field is unused.
No row is promoted on negative grep evidence. Full upstream attempt identity,
stream metadata/order, worktree failure cleanup, startup native discovery/settings
and daily chat features listed in IMPLEMENTATION remain open. P0 stays 0 done / 5
partial; do not describe this as complete daily-use parity.

## Client-read audit update (2026-11-01)

`tools/audit-client-fields` now scans every retained P0/P1 method difference,
including camel/Pascal/snake/kebab spellings across lib-t3-rust, desktop Rust/QML
and Android Kotlin. The 3991 report is historical input, not a fresh parity run.
`P0-FIELD-AUDIT.md` records reader candidates for every differing field name.

Accepted differences: `latestLocalTurnOrdinal`, `latestVisibleMessage`,
`lastErrorCode`, and `pendingGuestMessageCount` have no reader in any of those
three clients (including alternate spellings). Their absence does not block done.
These are projection convenience metadata, not substitutes for the consumed
`latestRun*`, message arrays, runtime requests or error/status fields. Capability
keys without a literal hit are **not** accepted wholesale: clients also derive
capabilities through enums and generic maps.

All five P0 RPCs still have consumed gaps, so remain partial. Examples:
`activeAttemptId` is read by lib `queue.rs`; provider-thread/run identity by
`timeline.rs`/`queue.rs`; repositoryIdentity by client project/VCS views; quota
`checkedAt`/windows by `quota_sources.rs` and `quota_present.rs`; settings and
model metadata by desktop/Android provider settings. An unused-field exception
cannot excuse those. P1 partial rows retain consumed gaps: asset kinds, rich find,
search result metadata, secret delivery, host adapters/PR status, settings, terminal
instance environment/history, fork native history and model handoff. No P1 row is
promoted based on negative grep or matching rejection probes.

New behavior: durable exact-operation approval grants are stored in SQLite in the
same receipt transaction. Matching native requests in the same thread, provider,
project and worktree use the ordinary runtime-response command/outbox path. Grants
never cover questions or mode transitions; unsupported generic Codex operations
fail closed. This is narrower than native wildcard permission rules and has no
revocation UI. DB reopen and changed-command behavior have real-client coverage.

Claude ExitPlanMode captures native `input.plan` (also record-wrapped), denies
immediate implementation and waits for a later user turn, matching pinned adapter
behavior. Both providers project proposed plans; the real client plan helper then
switches interaction mode and dispatches implementation with sourcePlanRef. The
source plan becomes completed atomically with the implementation message. Plan
feedback remains in plan mode. Peer E2E covers capture, completion, approval,
mode change and source-plan state; live plan inference is not claimed.

Claude observed rate-limit frames supply checkedAt and native session/weekly
windows; absent observations remain absent. Explicit title regeneration produces
a deterministic first-message title (80 characters), not a provider-generated
semantic title. Idle settlement retains failures, queued work, pending requests,
subagents, uncertain controls, pins, opt-outs and PR-linked threads. Automatic
merge settlement remains a gap.

## Client field implementation checkpoint (2026-11-01)

Local real-Connection tests now cover pending/active/final attempts and native
provider-thread links, run/node metadata on ordered replay, unique Codex item
ordinals, preserved native start times and context usage. Project mutation resolves
local Git repositoryIdentity; background resolution emits replacement snapshots
with resolvedRepositoryIdentityRoots, even for null/no-remote identities. Updates
use a compare check so an older resolution cannot overwrite newer stored metadata.
Force-delete marks child thread shells deleted in the same transaction.

Startup config lists filesystem skills and project commands before a turn, preserves
native discovery after init, exposes Codex reasoning choices, and advertises supported
thread actions. Settings apply favorite/hidden/order mutations rather than persisting
mutation instructions. Terminal open/attach/restart apply the selected instance's
unredacted environment with explicit terminal env taking precedence. Search supplies
snapshotSequence and supplemental thread shells, including older settled title rows
matched by environmentLabel. Git status includes recognized host descriptors and
GitHub PR fields; localOnly skips that host lookup. Signed asset tests exercise every
resource tag, including Linux native icons and credentialed GitHub media with a CLI peer.

Only projects.mutate is promoted in this checkpoint. Search results are consumed:
FTS-prefix matching is not equivalent to upstream substring/text ranking, so that row
stays partial even though its missing output fields are filled. Other rows retain
consumed gaps: pre-native provider-thread identity; complete background-task and quota
reset status; custom provider configuration; service-tier descriptors; native startup
probe completeness; scoped/background settings normalization; GitLab/self-hosted PR
adapters; MCP app HTML bridge; persistent terminal history; native fork/model handoff,
attachment steering and recovery/control acknowledgement. Empty optional metadata is
not proof that native observations are projected.

Reader checks used `rg -n` on `activeAttemptId` in lib-t3-rust/src/queue.rs;
`contextUsage` in lib-t3-rust/src/timeline.rs and desktop src/lib.rs;
`pendingBackgroundTasks` in lib-t3-rust/src/sidebar.rs and desktop src/lib.rs;
`threads`/`snapshotSequence` in desktop src/shell_state.rs:62 and src/thread_search.rs;
asset tags in lib-t3-rust/src/api/assets.rs; model preferences in
lib-t3-rust/src/models.rs; and MessageSearch.kt in Android core/home. These are positive
reader evidence, not accepted omissions. No new unused-field exception is claimed;
the four previously accepted omissions above retain their existing audit evidence.
The historical upstream differential was not rerun by this checkpoint.

Memory follow-up: replacement snapshots reuse the bounded disk writer. The first
materialized-shell implementation failed the real-profile benchmark (72.36 MB HWM)
and was corrected; the final profile probe passes at 46.24 MB HWM. See RSS.md for
limits and raw aggregate artifacts. Snapshot replacement can coalesce intermediate
shell updates; its sequence boundary prevents replaying those updates afterward.

## P0/P1 reader sweep (2026-10-31)

For each P0/P1 row not done, the remaining differences were grepped (`rg -n`, all spellings)
in lib-t3-rust (`src`, `bindings`), omarchy-t3-code (`src`, `ui`) and ot3-android (`app`, `core`).
Only a difference no client reads is accepted; a read keeps the row partial.

Accepted differences (only secrets.answerRequest is promoted):
- `secrets.answerRequest`: agent-side consumption of the saved secret file. Readers of
  `secret_request` are `lib-t3-rust/src/timeline.rs:297`, `omarchy-t3-code/src/lib.rs:572` and
  `write.rs:456` (secret-answer); they use the item status and a `null` reply, never the stored
  value or its path. Nothing in the three clients reads the file. Caveat: no o3 adapter emits
  `secret_request` items yet, so an agent cannot ask for a secret; the RPC itself is conformant.
- `git.runStackedAction` hook events (`hook_started|hook_output|hook_finished`): only
  `lib-t3-rust/src/api/streams.rs:321` names them in a tag list; no desktop or Android hit.
  Not enough to promote the row, because clients read the generated commit message path
  (both UIs label the message "optional") and cancellation.

Implemented because a client reads it:
- `server.discoverSourceControl` now reports GitHub, GitLab and Azure DevOps CLI descriptors with
  read-only auth status (`omarchy-t3-code/src/write.rs:75` reads `kind`, `status`, `auth.status`).
- `subscribeServerConfig` sends the typed `providerStatuses` event for provider-only changes
  (`lib-t3-rust/src/quota_sources.rs:127`), covered in `tests/thread_commands.rs`.

Kept partial, reader and reason:
- discoverSourceControl: `T3AddProject.qml:50` lists Bitbucket/Forgejo/GitCafe, API-token providers are not reported.
- subscribeServerConfig: `usageLimitSourcesUpdated` (Android `NativeT3Transport.kt:181`) has no source.
- assets.createUrl: MCP app HTML (`omarchy-t3-code/src/write.rs:399` mcp-app) and icon theme discovery.
- git.runStackedAction: both git UIs expect generated messages when blank; cancel does not stop a step.
- orchestration.dispatchCommand / launchThread / subscribeShell / subscribeThread / getConfig, message.dispatch,
  run.interrupt, runtime-request.respond: consumed gaps listed under Client-read audit update.
- searchThread/searchThreads: Android `MessageSearch.kt` reads match segmentation and ranking.
- subscribeCommandOutput: Claude output arrives at tool completion, not streaming.
- terminal.open/attach: closed by the bounded history recovery evidence below.
- thread.user-input.dismiss: native delivery to the provider and recovery are unfinished (`lib-t3-rust/src/questions.rs:48`).
- thread.fork, thread.model-selection.set: native resume-at mapping and cross-provider handoff.
- updateSettings, vcs.refreshStatus: settings normalization; GitLab/self-hosted change requests.

## Stacked-action reader closure (2026-11-01)

`tests/vcs_compat.rs::stacked_blank_message_is_scoped_and_cancel_stops_hook`
uses the pinned real Connection with directory-isolated Git state. A whitespace
message deterministically becomes `Update <path>` (or a file count); selected
paths exclude unrelated staged files from that subject. No inference is needed
for this fallback. Closing/interruption of the subscription drops the producer;
subprocess process groups stop the running Git step and its hook descendants.
The test cancels inside a sleeping pre-commit hook and verifies both unchanged
HEAD and absence of its delayed marker. Existing tests cover positive stacked
commit/push/PR paths and error receipts. `git.runStackedAction` is promoted using
that evidence and the hook-event unused-reader exception documented above.
Items 2–5 of the requested follow-up remain pending; no other row is promoted.

Validation: full default-feature suite 142 passed / 5 opt-in ignored; strict
all-target clippy passed. `tools/bench-rss` completed with bridge enabled,
1000 messages, 20 threads, 3 subscribers and 1000 unary calls: VmHWM 40692 KiB
(41.67 MB), below 50 MB. Inference excluded. Aggregate output retained locally
at `/var/tmp/o3-build/sweep-rss.log`; this is the synthetic workload, not a new
real-profile measurement.


## Terminal history reader closure (2026-11-01)

The terminal readers consume snapshot history and live output
(`lib-t3-rust/src/domain/terminal.rs`,
`ot3-android/core/src/main/kotlin/dev/avillagran/ot3android/core/terminal/TerminalSession.kt`).
Atomic snapshots in the private profile retain at most 512 KiB / 5000 lines per
terminal. Startup restores stopped snapshots, never PIDs or provider environment.
Open preserves recovered history; explicit restart and changed cwd/environment
still clear it. Clear persists an empty history; close and inactive eviction remove
the saved snapshot. Recovery is server-restart persistence, not power-loss durability.
The real-client test
`bounded_history_survives_state_reopen_for_attach_and_open` reconstructs state from
disk and verifies the trimmed tail, stopped attach, open, clear and close.
Both P1 rows are promoted by this reader evidence.

## Unreproduced VCS test failure (2026-11-01)

`pull_reports_state_and_branch_changes_against_the_default_branch` failed once
in a full suite run on `b16facb`. It could not be reproduced in 66 stress runs
or three full suite runs. Cause unknown.

Terminal closure validation: 143 tests passed, five opt-in tests ignored; strict
all-target/all-feature clippy, format and diff checks passed. The completed
release RSS probe (bridge, 20 threads, 1000 messages, three subscribers, no
inference) measured 39164 KiB HWM, below 50 MB.

## Typed config deltas (2026-11-01)
Provider-only, settings-only and keybinding-only changes now use typed payloads;
coalesced unrelated changes retain a replacement snapshot. Real Connection test
`config_stream_sends_typed_provider_and_settings_updates` checks settings against
getSettings and applies provider events through quota_sources. Reference reader:
client-runtime/src/state/serverConfigProjection.ts reads payload.settings and
payload.keybindings/issues. Row remains partial: quota sources and environment
themes are not projected yet.

## Search title coverage (2026-11-01)
Default search now includes title substring matches, ranks them ahead of message
hits, and folds Greek final sigma consistently with searchThread. Real Connection
`search_titles_participate_in_default_search_with_folded_substrings` covers a
body-only competitor, an interior title substring, case folding and titlesOnly.
Both search rows remain partial: complete upstream ranking, body substring/folding
and rendered/context segmentation still require implementation and reader evidence.

## Add-project discovery entries (2026-11-01)
Real Connection discovery test now checks Forgejo/Gitea (`tea`), GitCafe (`cafe`)
and Bitbucket (API, no executable), alongside existing CLI providers. These are
the missing kinds listed by desktop T3AddProject.qml. Bitbucket availability is
independent of CLI installation, matching reference discovery.ts's API branch.
Row remains partial: credential/API validation and fj fallback are unavailable;
new host authentication stays explicitly unknown rather than inferred from exits.

Validation of reader follow-up: targeted real-client tests pass; full suite 144
passed, 5 opt-in ignored (initial resolver-fixture ETXTBSY; complete rerun passed).
fmt/diff checks pass. tools/gen-compat regenerated. tools/bench-rss completed with
bridge enabled, 1000 messages, 3 subscribers, no inference: HWM 41896 KiB
(42.90 MB), below 50 MB. No rows promoted; P1 methods remain 23/31 done.

## Launch workspace rollback (2026-11-01)

`launch_worktree_failure_rolls_back_and_reuse_updates_workspace_only` uses the
real Connection and an isolated Git repository. An injected run-insert failure
removes only the newly created worktree and branch; retry creates one durable
launch. Existing worktrees are never cleanup-owned. Launches serialize preparation
and receipts so concurrent retries cannot remove another successful launch's tree.
Cleanup errors are included in the launch error. Reuse requires an empty, active
thread in the same project and records workspace metadata, retaining title/model
as pinned ThreadLaunchService.ts does before initial-message dispatch.

The row stays partial: the initial-message model override on reuse, imported-native
launch and generateTitle preparation still need parity. Readers can send these
through lib-t3-rust/src/api/orchestration.rs::ThreadLaunch and desktop src/write.rs.
No crash-atomic filesystem/SQLite transaction is claimed.

## Stream reader contract (2026-11-01)

`lib-t3-rust/src/stream.rs::{cursor_of,advance,start_of}` uses a snapshot's
boundary as the applied cursor and advances event cursors monotonically. Its
`src/projection.rs::apply` applies ordered row updates to a bounded mirror.
Desktop `src/shell_state.rs` replaces shell rows; the reader does not demand a
particular relative order between independent operations on different records.

The real-client test `snapshot_tail_and_resume_preserve_client_cursors_and_projection`
checks both shell and thread subscriptions: snapshot first, strictly increasing
tail sequences beyond its boundary, identical ordered replay from that boundary,
and final state equal to a fresh read after pin/unpin/pin. It applies thread events
using the pinned client's actual projection reducer and cursor functions. Existing
`streamed_full_and_compact_snapshots_decode_with_real_client` and
`project_identity_refresh_replaces_shell_without_losing_threads` cover snapshot decoding
and replacement boundaries. This is bounded deterministic coverage, not exhaustive
scheduling proof.

Accepted difference: the relative interleaving of independent operations need not
match upstream, provided snapshot-before-tail, monotonic sequences, replay order
and per-record causal order hold. This exception does not permit stale updates,
dropped required records, or advancing past data absent from a replacement snapshot.
The shell/thread rows remain partial for their explicit background/identity/quota
gaps; a generic demand for all interleavings is no longer a reason for partial.

## P0 decision after launch and stream fixes (2026-11-01)

All five rows were reconsidered under the reader rule; none is promoted:

| Row | Remaining consumed behavior |
|---|---|
| dispatchCommand | Native background ownership/settlement, attachment steering and delivery acknowledgement; broad permission rules/revocation. `lib-t3-rust/src/queue.rs`, `sidebar.rs`, `approvals.rs` consume execution/approval state. |
| launchThread | Reused initial-message model override and generateTitle preparation use the launch contract. importedNativeThread is a server extension on launch, absent from the client builder; it is not reader-equivalence evidence. Workspace rollback and empty-thread reuse now have positive Connection coverage. |
| subscribeShell | Native background tasks and provider quota failure/reset. Desktop `src/lib.rs:674` holds background work and `:731` reads `usageLimitResetAt`. |
| subscribeThread | Pre-binding identity and native background lifecycle/recovery. `lib-t3-rust/src/queue.rs:7`, `timeline.rs:227` and desktop `src/lib.rs:511` resolve the active provider thread. |
| getConfig | Custom instances, complete startup native discovery, tier descriptors, proactive quota refresh and settings normalization. `lib-t3-rust/src/provider_instances.rs`, `scoped_settings.rs` and `native_settings.rs` read instance configuration and scoped overrides. |

Stream interleavings are governed by the explicit reader contract above rather than
an unbounded equivalence claim. Exact original-inference reattachment is not promised:
RECOVERY.md documents native continuation as a new turn. Remaining native control
uncertainty and client-visible background recovery are still actual gaps.


## Native background ownership and pre-binding identity (2026-11-01)

Reader audit: `lib-t3-rust/src/sidebar.rs`, `sidebar/view.rs`, `awareness.rs`
consume pendingBackgroundTasks/activityRunStatus/blockedOnThreadWait;
`queue.rs`, `timeline.rs`, `compaction.rs`, `checkpoint_revert.rs` resolve
activeProviderThreadId through providerThreads. Desktop `src/lib.rs` and
`session_memory.rs` consume the same projections. Android uses the shared Rust
reducers (its Kotlin UI does not introduce a separate native task roster).
Attempts and nodes must retain the reserved provider-thread link before native init.
Reference: ClaudeAdapterV2 task/roster handling and background Agent launch ACK;
CodexAdapterV2 retains turn routes for late command items; Orchestrator derives
pending work from the provider roster and child lifecycle.

`tests/background.rs` uses the real protocol Connection and gated stdio peers.
Before native init, the snapshot contains a non-null activeProviderThreadId and
matching providerThreads/nodes/attempts, with a null nativeThreadRef. Binding updates
that record in place. Claude Bash and Task launch ACKs and Codex command items can
outlive the native result: run/root/attempt stay active, shell and thread snapshots
show owned work, late completion updates the streamed roster, and interrupt closes
ownership. SQLite restart tests fail lost work with an explicit reason; a reserved
ID alone cannot trigger native resume. Existing SIGKILL continuation peers and stream
reducer/cursor/replay tests still apply. Exact original-process reattachment remains
excluded by RECOVERY.md, rather than advertised as recovered background execution.

One opt-in real Claude Haiku background-shell check passed: launch `sleep 2` with
run_in_background, receive native task start/end, complete only after notification.
No real exhausted-quota failure was induced. Claude rejection resets are projected
from native rate_limit_event, but live rejection and Codex reset handling remain
unproven. Thus subscribeThread is promoted under the reader rule; subscribeShell
stays partial for quota failure/reset. Earlier P0 tables describe earlier checkpoints.

## Config and settings reader checkpoint, 2026-11-01

This checkpoint supersedes the config/settings open items in the older checkpoints.
Scope: `server.getConfig`, `server.updateSettings`, `subscribeServerConfig` (both
subscription spellings). The reproducible, per-field inventory is
[CONFIG-READER-AUDIT.md](CONFIG-READER-AUDIT.md), generated with
`tools/audit-config-readers` over shared-library `src`, desktop `src`/`ui`, and
Android production Kotlin/Rust sources. It records 198 fields and alternative
snake/kebab spellings. Grep locations alone are not semantic-reader evidence.

Positive reader evidence:
- `lib-t3-rust/src/provider_instances.rs:visible_instance/mutation` reads configured
  instance identity, enabled state, binary/home/launch fields, environment redaction,
  status and accent. Four real Connection launches use independent Claude/Codex
  instance pools and a native peer that asserts its home and environment. Removing
  an instance removes its descriptor; replacing settings retires its pool without
  redirecting an already running turn. Setup subprocesses use the same environment.
  Native/default aliases remain supported; disabled instances reject execution.
- `lib-t3-rust/src/models.rs`, desktop `T3ModelOptions.qml` and Android
  `ModelPresentation.kt` read model names and generic `optionDescriptors`, selected
  values and defaults. Native Codex model discovery supplies reasoning and service
  tier descriptors; the peer asserts `turn/start.serviceTier`. Native discovery uses
  a separate process from inference/recovery and reaps it, with no user prompt.
  Claude initializes the native control transport with hooks, MCP auto-connect and
  session persistence disabled for discovery; it reads native commands/account
  metadata. Native skills and the instance/user/project filesystem catalog are
  combined. The fresh configured-instance test proves these catalogs exist before
  its first inference request. Editor availability comes from executable discovery,
  consumed by `lib-t3-rust/src/thread_details.rs:editors`.
- `lib-t3-rust/src/native_settings.rs`, `settings.rs`, `storage_settings.rs` and
  `scoped_settings.rs`, desktop `T3Preferences.qml`, and Android scoped/general
  settings consume the normalized settings representation. Tests cover model
  preference mutations, partial text-generation selection, instance enable folding,
  null/entry replacement and redaction round trips, idle-compaction compatibility,
  background presets/custom overrides and derived legacy fields, writing-style and
  observability defaults, trimmed strings, scoped project overrides and atomic
  rejection. Unknown future keys continue to round trip.
- `lib-t3-rust/src/quota_sources.rs:apply/limits/sources` and `quota_present.rs`,
  desktop `src/quota_sources.rs`, and Android `NativeT3Transport.kt:181`,
  `UsageLimits.kt`/`QuotaStrip.kt` consume the snapshot/provider/settings events and
  opt into `usageLimitSourcesUpdated`. Replacement snapshots replay the last extension observations so the real quota
  reducer cannot silently clear their state. A local HTTP hub exercises the actual
  management `auth-files`/`api-call` protocol, filtering disabled accounts,
  Codex windows/reset time/credit count and failure projection. The test applies
  the emitted event through the pinned real client's quota reducer and checks that
  management credentials and redemption ids are absent from its public projection.
  Native quotas refresh proactively only with `usageLimitSources:true`; settings
  changes and the resolved health interval trigger refresh. Cancellation/socket
  teardown aborts its task and owned request. Codex quota probes reap their separate
  process. No quota polling task, timer, or request exists without an opted-in
  subscription; theme-only subscriptions never refresh quotas.
- Typed settings/keybinding/provider deltas retain a replacement snapshot for
  coalesced changes. `environmentThemesUpdated` and `usageLimitSourcesUpdated`
  are strictly opt-in, emitted after the initial snapshot. A guarded, bounded theme
  reader detects creation/removal, rejects symlinks/FIFOs and sends replacement
  arrays. Connection tests prove initial ordering, live theme replacement, quota
  cancellation and that legacy subscribers do not receive extension events.

Accepted differences under this reader scope (not blanket upstream parity):
- Auth discovery hints `bootstrapMethods`, `sessionMethods`, `sessionCookieName`
  and diagnostics hints `logsDirectoryPath`, `localTracingEnabled`,
  `otlpTracesEnabled`, `otlpMetricsEnabled`, `otlpLogsEnabled` have no literal
  reader in these clients (see the complete zero-match list in the inventory).
  Bearer authentication remains covered by its existing real-client tests;
  diagnostics hints stay conservative. Model `promptInjectedValues` has no literal
  reader either; the pinned Claude manifest remains intact.
- `directEndpoints`, `nativeSessions`, `canList`, `canResume`,
  `reportsContextWindow`, `threadSnapshotPagination`: no client field reads in the
  inventory. Existing conservative capability values remain; no paging/import or
  native-session-list capability is inferred from this checkpoint.
- `defaultThemeSetAt`, `cursorKeychainUsageEnabled`, `customModels`,
  `previousWorktreesDirectories`, `usagePriceOverrides`, `usageModelAliases` and
  `sourceControlHosts`: no direct production reader in these clients. Preserve
  settings round trips; this checkpoint does not claim their unrelated execution
  services. Default themes continue to round trip; Android's `Theme.kt` matches
  theme names locally, and its `OmarchyThemeCatalog.kt` loads bundled assets.
- `automaticGitFetchInterval`, `providerHealthRefreshInterval` and legacy project
  browser/auto-pull maps have no direct reader; they are nevertheless derived from
  the settings that native settings/scoped settings do read. Background host power
  monitoring and Git fetch execution are separate operations, not config-reader
  guarantees. Source quotas honor the normalized refresh interval.
- `serviceTier`, `serviceTiers`, `defaultServiceTier` have no literal-name client
  reads, but **are not omitted**: generic option descriptor readers consume their
  model options. Native metadata is preserved rather than inventing tier support
  for models that do not advertise it. `isCustom`/model aliases are not literal
  reader dependencies; native/custom model identifiers are passed through.
- Theme subscription flags/tags are exposed by `lib-t3-rust/src/api/streams.rs`
  but no native client applies these remote palette events. Android's
  `EnvironmentThemeReader.kt` separately reads host theme files through filesystem
  RPCs, and also uses its bundled theme catalog. The opt-in event is implemented/tested, while full upstream
  palette validation and reserved built-in palette ids beyond system/light/dark
  remain outside reader equivalence.
- Other driver families remain explicitly unavailable. The clients already gate
  their operations on provider availability/install/setup capability; these rows
  do not claim adding inference adapters or managed installers for those families.
  Native version advisories/update installation are likewise capability gated.

The three rows are promoted for these actual reader paths, not for matching negative
probes. Native peers are deterministic protocol evidence, not a claim of fresh
upstream inference or plan-exhaustion testing. Existing live inference evidence and
remaining shell quota/reset/recovery limits retain their earlier scope.

Final validation for this checkpoint: 156 default / 154 no-default tests pass,
six opt-in tests ignored in each configuration; strict all-target Clippy passes in
both configurations, as do formatting, generator and diff checks. Five added
Connection tests include failed native discovery reporting an installed binary
honestly, without claiming it is healthy. `tools/bench-rss` completed with HWM
15780 KiB (16.16 MB), below 50 MB; the sanitized aggregate is
[rss-config.json](rss-config.json). This workload excludes inference and its child
process RSS; it does not certify live provider quota exhaustion/reset. No emulator,
service/profile deployment or public route change was performed.

## Launch and dispatch reader closure (2026-11-01)

This checkpoint supersedes the launch/dispatch gaps in earlier tables.
`tests/orchestration_readers.rs` uses the pinned real Connection and native stdio
peers for both providers. Reused launch applies the requested model before staging
the initial run; the peer asserts the override on Codex turn/start. Imported strong
native references and metadata are stored before the initial run, and Codex resumes
the imported id. Launch receipt replay returns the original result. generateTitle
uses an isolated bounded provider turn with a deterministic fallback, as does
thread.metadata.update; semantic titles and their input settings are client-read
behavior and are not an accepted difference. Existing provider_input tests cover workspace rollback and occupied reuse.
Direct and queued-to-active steering now deliver uploaded image/file bytes and
serialized context through Claude user content and Codex turn/steer. Invalid/missing
uploads fail before the durable control handoff. Message projections retain uploads.

Permission reader audit (read-only source check): shared `src/approvals.rs:7,17,27,73`
reads live runtimeRequests, approval_request items, offered decisions and response
capability; `src/api/orchestration.rs:462,550,850` validates decisions. Desktop
`src/write.rs:163,173,198,211` uses that reducer and sends thread.runtime-mode.set
before dispatch when the permission selector changes. Android uses shared reducers.
Search terms: `approvalPolicies|permissionRule|revoke|acceptAlways|acceptForSession`, across shared
Rust src, desktop src/ui and Android app/core src. Auth/device/bridge revocation
hits are distinct APIs and unchanged. No inference-policy CRUD/revocation command
is exposed by these readers. The client-offered `acceptForSession` now stores exact-operation consent in
SQLite, scoped by thread/provider/project/worktree, independently of native
process lifetime. `acceptAlways` remains a server extension, not an offered
default client decision. This is not evidence of broad native glob rules; generic questions and mode changes cannot acquire wildcard consent. Changing
runtime mode or model selection now deletes previous thread consent, so restoring
an earlier mode cannot silently restore grants. `provider_input` proves persistence,
automatic repeat acceptance and a fresh pending approval after mode revocation.
Native plan acceptance and structured-question tests still apply.

Accepted recovery boundary for dispatch: receipts acknowledge durable acceptance,
not original-process native delivery. RECOVERY.md and the outbox retain uncertain
controls without blind replay; active_recovery tests prove explicit new-turn
continuation. No exact original-inference reattachment is claimed. With the positive
reader evidence above, dispatchCommand and launchThread are promoted to done.

Codex rate-limit windows now project the latest exhausted-window reset to shell;
non-exhausted windows clear it, while sparse metadata-only updates preserve it.
A failed turn with codexErrorInfo=usageLimitExceeded sets lastErrorClass=usage_limit;
the next successful run clears the shell failure class. Claude rejected rate limits
also mark that run's error class. `codex_quota_failure_and_reset_reach_shell_reader`
proves both fields through real subscribeShell snapshots using a schema-based
synthetic stdio peer. This is **not** an exhausted-account capture. The repository's
real Codex capture (15 frames in real-0.166.0.ndjson) contains no usage-limit failure, and the reference fixtures have
only synthesized projection errors. The required realistic captured failure frame
is unavailable: subscribeShell remains partial pending that evidence. No credentials
or private live profiles were inspected to manufacture quota exhaustion.

Validation: 158 default / 156 no-default all-target tests pass (six explicit opt-in
ignores each); strict Clippy in both configurations, fmt, gen-compat and diff checks.
`tools/bench-rss` completed with bridge enabled: HWM 15,804 KiB (16.18 MB), below
50 MB; 1,000 messages, 20 threads, three subscribers and 1,000 unary calls, inference
excluded. Sanitized aggregate evidence: [rss-orchestration.json](rss-orchestration.json).

### Reader execution baseline (2026-11-01)

`tests/client_readers.rs` drives a real server with `ThreadLaunch` and `Command`
from lib-t3-rust origin/main `7cbad5d8198d37e9ded767d33ac03f2cf15875f4`.
It executes approvals (including the validated response builder), queue, timeline,
sidebar, awareness, provider_instances, scoped_settings, storage_settings and
models::catalog on server output. Empty-state checks alone do not establish full
parity; the five independently demoted rows remain partial.

OmaBridge's old protocol dependency is patched to a small compatibility facade:
it reexports the same current library (one FFI implementation), retaining only
its removed MIT bridge_catalog module from `8c80c4d`. No client reader is copied
or replaced by the facade. The current library is pinned by revision.

### Capability gates

The descriptor (also embedded in getConfig and subscribeServerConfig) now advertises
attachment/file uploads and inline context (`asset_kinds`, `provider_input`), PR
reads/checks/stack actions (`pull_requests`, `vcs_compat`), settlement and ordering
(`thread_commands`), storage/scoped settings (`client_readers`), quota/themes
(`server_config`), OTLP probes (`admin_reads`) and command output (`command_output`).
The real scoped writer now writes and resolves a project override and changes the
worktree directory; cleanup invocation is also exercised. The later storage-policy implementation supersedes this early limitation: both
`storageCleanup` and `projectWorktreeCleanup` are true and execute owned-worktree
policies; the collector releases the database lock around Git subprocesses.

### Provider setup reader closure

The real `provider_instances::projection` is asserted for configured Claude/Codex
instances: sign-in/logout are enabled only for installed supported drivers,
Claude exposes its native updater, and install remains false (the RPC explicitly
rejects managed installation). Version status is truthfully `unknown`, retaining
the installed version without inventing a latest version. Update state transitions
through running and succeeded/failed. Latest-version background checks remain an
open settings gap; capability gating is not an accepted substitute for that work.

### Title generation contract

`tests/title_generation.rs` uses the actual launch/action builders and checks
initial and regenerated semantic titles, configured model/options/instructions,
failure fallback, and a rename racing generation. It applies the server's real
metadata events through `projection::apply`, observing request/start and completion
in `titleRegeneration`. Completion uses `thread.title.regeneration.complete` with
a matching requestId; stale completion cannot overwrite a manual rename. Each
short provider turn is independent of the conversation native session, capped at
20 seconds / 4 KiB output, and never answers tool/permission requests.

`tests/session_approvals.rs` builds the first response with the real
`Command::approve`/`approvals::command`, selecting `acceptForSession`. Three
separate native processes are observed and reaped by the actual idle timer.
The second identical operation completes without UI intervention; a changed
operation remains pending and requires a fresh client approval. All resulting
pending/cleared states are consumed by `approvals::pending`.

### Effective background activity

`backgroundActivityProfile` now controls real periodic provider health probes,
including with no quota subscription. `tests/background_profile.rs` writes presets
with `native_settings::patch`, reads them back with `native_settings::snapshot`,
and verifies refresh-driven version changes via `provider_instances::projection`.
An accelerated custom interval fires; zero stops it; restoring it restarts it.
Battery/performance resolve to 900,000/60,000 ms respectively. This does not claim
automatic git fetch or host-power monitoring; those settings remain an open gap.

### Final independent-audit follow-up

Both inventory tools scan all text file types, hidden/ignored files, bindings and
data, in deterministic path order. Config inventory also discovers the descriptor
keys, avoiding a stale capability allowlist. Fixture/descriptor matches are only
candidates and do not establish production consumption.

No demoted row is promoted by this follow-up. The reader sweep is **not clean for
promotion**: storage/worktree policy execution, latest-version checks, automatic
git/power activity, source-control/review generation settings, and imported native
references on `thread.create` still need closure. Durable control receipts also
retain the documented delivery-uncertain boundary. `subscribeThread` keeps its
prior status; no new concurrent multi-record equivalence is claimed here.

### Native create import reader closure (2026-11-01)

`thread.create` now shares launch's native-reference validation and persistence in
its receipt transaction. `imported_create_is_atomic_idempotent_and_readable` uses
the real `ThreadCreate` builder, reads the provider record through
`thread_projection`, and executes timeline and queue readers. A repeated command
creates one binding; a mismatched native driver leaves no thread. This closes the
create-import gap, without claiming native execution recovery or promoting
`dispatchCommand` past its documented uncertain-delivery boundary.

### Provider version checks (2026-11-01)

Manual provider refresh and the health timer now read bounded npm latest metadata
for installed Claude/Codex drivers. The advisory uses client values `behind_latest`,
`current`, and `unknown`; failed/malformed lookups never manufacture a latest
version. `enableProviderUpdateChecks=false` stops lookups and hides cached advice.
`tests/provider_versions.rs` drives real server output through
`provider_instances::projection` for newer, equal, failed and disabled checks,
using a local metadata server. No package is installed by this check.
Config/settings rows remain partial pending the other execution gaps.

### Storage policy execution (2026-11-01)

Cleanup now applies configured log/browser-artifact retention and global/project
worktree policies, with a periodic collector for enabled policies. Worktree age,
settled age, deletion, merge ancestry and unchanged-tree triggers use actual Git
state. Only worktrees created by this profile are eligible; imported/existing,
shared, pinned, busy, uncertain-control, dirty and Git-locked worktrees are kept.
Git collection never uses force, and probes have bounded time/output. Successful
removal clears the thread path and emits metadata for the client projection.
The manual log-cleanup default remains 14 days when no retention is configured.

`tests/storage_policies.rs` uses real storage writers and resolved rules, creates
real worktrees through launch, and exercises timeline on collected projections.
It verifies collection of clean/deleted trees and old log/browser files while
retaining dirty, pinned, shared and running trees. Cleanup capability gates are
now enabled. This closes policy execution; the broader settings rows remain
partial for activity and source-control/review generation.

### Git and power activity execution (2026-11-01)

Background profiles now schedule real serialized `git fetch --all --prune` for
registered projects. The collector reads Linux battery status/capacity and the
current login session's LockedHint on the configured active/idle monitor interval;
unknown host observations do not invent a lock or battery state. Profile pause
flags suspend Git/provider work for host lock, low battery, battery discharge,
and unexpired visible client low-power reports. Zero intervals disable the job.
Client TTL defaults follow the profile and are clamped to 1–120 seconds.

`tests/activity_git.rs` writes profiles through the real native settings writer,
verifies their reader projection, and proves actual remote-ref advancement after
battery pause/resume using isolated repositories. Fetch leaves HEAD unchanged.
`tests/background_profile.rs` uses the actual ClientActivity builder to pause and
resume health probes as well as checking version output with the provider reader.
The cleanup timer also recognizes a global custom worktree policy.
Settings/config remain partial for source-control/review generation and the final
independent reader sweep.

### Source-control generation execution (2026-11-01)

Generated commit messages now use sourceControlWriterModelSelection (falling back
to textGenerationModelSelection), including model options and scoped settings.
The bounded turn applies custom writing instructions or recent repository subject
conventions. Explicit messages win; failed/tool-requesting generation retains the
deterministic filename fallback. Semantic/custom feature branches use branch
instructions and the configured prefix. PR body generation includes bounded
branch diff context and, when enabled, a repository-local PR template. Each turn
has no native conversation binding, a 20-second limit and a 4 KiB output cap.

Review-guide generation/read paths now use real PR diffs and persist the resulting
text; enablement, review model selection and review instructions are enforced.
These guide RPCs have no production client call sites and retain their unused
status; adding them is not evidence of a nonexistent guide-output reader.
`tests/source_generation.rs` executes actual native settings writers/readers and
the lib's StackedAction subscription decoder on generated commit output. It checks
model/options/instructions, actual Git commit body, fallback, branch prefix and
semantic generation, PR template context, and generated/read-back review text.
The existing title-generation readers remain exercised after sharing the bounded
turn implementation. No config/settings promotion is made by this item alone.

### Final client-read decisions (2026-11-01)

The earlier open-gap lists above are historical. This sweep supersedes their
storage, version-check, activity, generation and create-import entries:

| Row | Decision | Reader evidence / remaining boundary |
|---|---|---|
| orchestration.launchThread | Promote | Actual ThreadLaunch creation, reuse and receipt replay; projection/timeline/queue/sidebar; title generation, rename race and fallback; native worktree rollback and steering peers remain covered by their integration tests. The launch-only native import extension is not used as client evidence. |
| server.getConfig | Keep partial (re-audit supersedes promotion) | Actual provider_instances, models, native/scoped settings and storage readers on unary output and real config snapshots; setup capability gates, version advisories and cleanup behavior covered by the new tests. Managed built-in installs and pinned updates now execute; external/custom setup and browser-service limits keep the row partial. |
| server.updateSettings | Keep partial (re-audit supersedes promotion) | Actual native/scoped/storage writers and readers drive policies, background Git/power activity and bounded title/SCM/review generation, including failure fallback and model options. |
| subscribeServerConfig | Keep partial (re-audit supersedes promotion) | The same config readers execute on its real snapshot; existing quota reducer tests cover providerStatuses and usageLimitSourcesUpdated; theme and settings delta tests remain green. |
| orchestration.dispatchCommand | Keep partial | Create-import gap closed with actual ThreadCreate/projection/timeline/queue. Native acknowledgement after a crash still has a delivery-uncertain boundary; no exactly-once control claim. |
| orchestration.subscribeShell | Keep partial, unchanged | No real exhausted-provider frame; synthetic failure evidence remains explicitly insufficient. |

Final collection refinements honor the selected keep policy for untracked/ignored
files while retaining tracked changes, locks, pins, sharing and pending work.
The collector skips the repository mutation lock rather than racing SCM operations;
its real-Git test verifies both that lock and Git's persistent worktree lock.
Automatic collection does not apply the manual log default to a disabled log policy,
and recognizes global custom policies. Successful provider updates refresh installed
versions and advisories before reporting the final configuration.

Inventories remain source-consumer inventories, not proof by grep: the decisions
above rely on executed client readers and the behavior tests described in each
closure. No other partial RPC or command is promoted by this sweep.

Reusing an empty owned thread with an existing workspace clears ownership unless
the path is unchanged. The storage integration test verifies that this cannot
make an external worktree eligible for collection. The activity test also reads
`behindCount`/`aheadCount` through the actual VCS refresh API after automatic fetch.

Final validation: 168 default / 166 no-default all-target tests pass, with six
explicit opt-ins ignored per configuration; strict Clippy passes in both feature
configurations, as do formatting and diff checks. The inventories were rerun:
333 client fields and 216 config fields. `tools/gen-compat` yields 86 done,
32 partial and 85 unused RPC rows (P0: 3 done / 2 partial; P1: 25 / 6).
Both isolated short native Claude/Codex turns passed via `tools/live-chat-e2e`.

Release `tools/bench-rss` completed with bridge enabled: HWM 15748 KiB (16.13 MB), below 50 MB; 1,000 messages, 20 threads, three subscribers and 1,000 unary calls. Provider registration/probes are included; inference is excluded. Aggregate evidence: [rss-orchestration.json](rss-orchestration.json).


### Safety and settings follow-up (2026-11-01)

No row is promoted by this work. The earlier promotion table and validation counts
are historical; `compat-status.json` is authoritative. Config and settings rows
remain partial until all client-read gaps have effect coverage.

- Text generation uses one active slot and at most four waiting callers, an empty
  data-directory cwd, a bounded lifetime, and thread-derived default selection.
  Claude CLI peers assert the exact tool-disable, MCP-disable, hook-disable and
  no-session-persistence argv; successful text processes exit rather than park.
  A live-child regression proves a colliding request cannot remove a conversation
  session from the pool.
  Codex generation **fails closed before process launch**: disabling shell and
  freeform patch flags does not prove that every native tool is absent. Titles
  and SCM use an explicitly selected safe text provider, otherwise their own thread's safe provider, otherwise deterministic local fallbacks. An installed unrelated provider is never selected implicitly. Normal Codex conversation turns remain
  supported. This is an explicit incomplete item, not a tool-free parity claim.
- Discovery waits for demand. Background health/version/fetch work sleeps without
  a connected RPC client or active run, and uses deadlines rather than a 100 ms
  timer. Fetch requires a reported project view, with T3’s 30-second balanced
  default. Settings cache invalidates on
  file change; checkpoint and identity workers suspend at idle. The idle poll test
  checks the background worker specifically, not intrinsic bridge heartbeats.
- Worktree collection reserves mutations separately, releases the DB before Git,
  and reacquires it to save results. `storage_lock` holds a slow Git peer while a
  projection read succeeds; `storage_policies` retains real-Git safety coverage.
- Recovery clears abandoned title busy flags. Limit failures with a known reset
  can snooze and retry according to settings; unknown reset times never cause an
  automatic retry. Automatic retries are new turns with an inspect-before-repeat
  instruction, not exactly-once execution. Restart continuation can be disabled.
- Response publication follows paragraph/turn selection; real native-settings
  writer coverage checks the intermediate and completed projections. Merge/squash
  credits are filtered with explicit gh subject/body arguments; rebase credit
  removal is refused. Worktrees initialize top-level/recursive submodules or skip
  them, inheriting project settings and t3.json. Full cross-provider reader effect
  coverage of these policies is still incomplete.
- No agent-browser service exists. `agentBrowserAccess:false` and `settingsSupport`
  explicitly report the browser setting and project override as unavailable. The
  added support fields are not proof that existing clients hide the control.
- Missing built-in CLIs can be installed with npm under versioned data-directory
  prefixes, with distinct empty npm config files and isolated HOME/cache. Codex scripts stay disabled; Claude native setup runs within its private prefix. A matching successful --version is mandatory. Updates switch the configured binary after success;
  old executables remain intact. Missing built-ins also accept pinned installation.
  Managed install/pinned-update argv and the real
  provider reader are tested. External/custom installations are not replaced;
  unsupported update/version/remove operations remain honestly unavailable.


Final follow-up validation: **177 default / 175 no-default tests pass**, with six
opt-ins ignored in each; strict all-target Clippy passes in both configurations,
formatting and diff checks pass, and `tools/gen-compat` completes. Isolated short
native Claude and Codex turns completed through the real client. Both release
RSS modes completed: **16,460 KiB (16.86 MB) synthetic** and **19,952 KiB (20.43 MB)
copied-profile** server HWM, excluding provider child RSS. The final copied-profile
snapshot was 497.63 ms. An earlier sample failed the tool's separate 500 ms latency
budget at 539.44 ms while remaining under the memory budget; this is not a claim
of invariant latency. Only aggregate measurements are published in
[rss-safety.json](rss-safety.json); private records and local source paths are omitted.


### Third-audit implementation validation

Signed file grants require filesystem scope, canonical registered boundaries, and
session validity at download. Profile and credential paths are excluded; symlink
changes and traversal are rejected by real-client regressions. Device captures
require operation scope and enabled device access; socket input rechecks disable
and session revocation. Public connections, headers, bodies and pairing attempts
are bounded, including upgraded connections.

Text generation never selects an unrelated provider implicitly. Safe configured
text selection takes precedence, then a safe thread-owned provider, then local
fallback. Installed CLI opt-ins exercise Claude text-only production argv, Codex
schema generation and handshake, and gh combined review/filter queries with API
debug evidence. Real npm installation of both built-ins passed in private prefixes
and both binaries executed --version; no real installation was updated.

Background ownership scenarios now run the actual lib projection reducer, timeline,
queue, sidebar and awareness readers before and after completion or interrupt;
restart also runs timeline and queue readers. The subscribeThread row remains
partial because broader upstream metadata and stream ordering differences remain.
No row is promoted to done solely from new transport assertions. Wireless ADB
serials with spaces and parentheses work in server discovery and detail RPCs; the
pinned lib's device-hub URL builder still rejects those serials.

Final implementation checks: 185 default tests and 183 no-default tests passed;
ten opt-ins were ignored in each default run. Five relevant opt-ins were then
executed successfully (real npm, gh API query, Claude text-only turn, Codex schema
and handshake). Strict all-target Clippy passed in both configurations; format,
diff and compatibility generation passed. Release bench-rss completed with
20 threads, 1000 messages, three subscribers and 1000 unary calls: VmHWM
16,044 KiB (16.43 MB), below both 50 MB and 50 MiB. Providers were registered
and probed, without inference; this is not a live-inference memory bound.

## Implementation follow-up (2026-11-01, after edca82f)

No compatibility row is promoted by this follow-up. The following is the complete
remaining P0/P1 inventory at that checkpoint, plus the two explicitly requested P2 rows.
The later Reader-gap implementation section supersedes this baseline inventory. A successful
transport call or scripted adapter does not close an unserved client reader.

| Surface | Precise remaining reason |
|---|---|
| `assets.createUrl` | MCP HTML resource grants do not implement full app HTML injection; native icon theme/platform discovery, media host parity and cache cleanup remain incomplete. |
| `orchestration.dispatchCommand` | `src/outbox.rs` persists a receipt and adapter-channel handoff, not native execution acknowledgement; `src/recovery.rs` reports uncertain delivery rather than replaying controls into a new turn. |
| `orchestration.searchThread` | `src/threadq.rs::search_thread` searches literal message text/plan markdown. It does not use the clients' rendered markdown/context/skills segments or folded-answer exclusions. |
| `orchestration.searchThreads` | `src/threadq.rs::match_expression` uses ANDed FTS token prefixes and bm25; clients' substring/folded-text and title ranking parity is unproved. |
| `orchestration.subscribeCommandOutput` | Claude Bash output is projected at tool completion, not streamed live; complete VT folding is unavailable. |
| `orchestration.subscribeShell` | No real captured exhausted-provider frame; quota/error reset coverage uses synthetic native frames. Retains its honest partial. |
| `orchestration.subscribeThread` | Owned-work readers run, but the retained real-T3 differential still differs in metadata/ordering. Completion replay on two real short turns does not certify concurrent stream interleavings. |
| `pullRequests.list` | One combined gh search expression was checked against real gh, but the full query/filter result space has no actual client-reader closure; other host adapters remain unsupported. |
| `server.discoverSourceControl` | GitHub/GitLab/Azure CLI descriptors exist; Bitbucket/Forgejo/GitCafe API-token providers offered by the desktop have no adapter. |
| `server.getConfig` | `settingsSupport` explicitly reports the unavailable agent browser executor. Custom/external CLI ownership/update paths and complete provider catalog metadata are not implemented. |
| `server.updateProvider` | Real isolated npm installation/version execution covers managed built-ins. Native `claude update` remains scripted-only; running it would mutate the installed CLI. Custom/external ownership remains unknown. |
| `server.updateSettings` | Browser access has no executor; exhaustive setting-effect reader coverage is absent. `src/codex.rs` refuses text generation because a tool-free transport has not been verified; no alternate provider is chosen. |
| `subscribeServerConfig` | Shares unary config limits; typed config/quota deltas do not supply the unavailable browser executor or external CLI management. |
| `vcs.refreshStatus` | GitLab/self-hosted change-request metadata and host error/status parity are unavailable. |
| `message.dispatch` | Attachment steering has deterministic native-peer coverage in `tests/orchestration_readers.rs`, not a real CLI capture. Exact native execution recovery and attempt identity across recovery remain incomplete. No blind replay or provider rerouting is introduced. |
| `run.interrupt` | Adapter channel reservation is not native acknowledgement. A crash between execution and durable acknowledgement cannot be resolved exactly once by the current native protocol. |
| `runtime-request.respond` | Broad approval-rule/revocation UI and restart acknowledgement are incomplete. Recovery cancels stale native approval channels and requires fresh authorization. |
| `thread.fork` | Latest native forks and explicit transcript handoff exist; native resume-at-message mapping for historical points is unavailable. |
| `thread.model-selection.set` | Same-session idle model changes exist; cross-provider context handoff is unavailable. |
| `thread.user-input.dismiss` | `src/events.rs` can cancel the local request and hand off a native response; restart cannot establish whether that native response was consumed. |

Test-race work:

- Terminal flow control now subscribes and acknowledges its initial frame before
  producing the burst. A separate producer-completion file separates a quiet
  socket from a finished shell. The post-ACK check uses the real terminal Output
  reader and split printf arguments, so input echo cannot satisfy the assertion.
  The retained old failure log reported zero received chunks; no production
  flow-control defect was reproduced and none is claimed fixed.
- The executable resolver test uses an installed ELF via a private symlink instead
  of immediately executing a newly written script. This removes the write/exec
  race from that fixture; it does not add retries to production process spawning.
  Concurrent fork inheritance of a writable descriptor is a possible ETXTBSY
  mechanism, not a proven diagnosis of the historical failure.
- The VCS pull test uses separate repositories, explicit main/upstream branches,
  real git and serialized server mutations. No reproducible failure or captured
  original assertion establishes a cause. Stress results below do not certify
  that the historical failure was fixed.

The isolated live test now feeds actual native Claude and Codex output from
subscribeThread replay through the pinned lib-t3-rust projection reducer,
timeline, queue, sidebar and awareness readers. Its assertions concern the
requested response and completed state, not fabricated native frames. These
short turns do not cover approval recovery, historical forks, exhausted quota,
all config fields, stream ordering parity or full interchangeability.

Follow-up validation: **185 default / 183 no-default tests passed**, with ten
explicit opt-ins ignored in each suite. Both strict all-target Clippy feature
configurations, format, diff checks and `tools/gen-compat` passed. The isolated
real Claude/Codex two-turn test passed separately with the new reader checks.
Targeted stress: terminal flow control **10/10**, real Git pull **20/20**, resolver
**20/20**; no VCS failure was reproduced. Release `tools/bench-rss` completed:
**16,432 KiB (16.83 MB) HWM**, below 50 MB, with 20 threads, 1000 messages,
three subscribers and 1000 unary calls. Inference is excluded from that workload;
provider child memory is excluded. Aggregate evidence: [rss-implementation.json](rss-implementation.json).

## Reader-gap implementation (2026-11-01)

1. Search keeps FTS candidates and verifies folded substrings, with a fallback
   capped at 4096 recent messages, 64K characters per fallback row and 8 MiB of
   candidate text per request. Titles use accent/case folding and exact/prefix/
   substring priority; single-project filters and subagent exclusion apply.
   Full palette word-boundary, recency, branch and pick scoring remains partial.
   The complete default all-target suite passed before this item's commit.

2. Thread find excludes `async-answer:<requestId>` rows folded into question
   answers, sorts visible positions and searches Markdown text in block segments,
   with hidden link destinations/context chips and displayed skill labels.
   Rich file labels, legacy context envelopes, citations, artifacts and the full
   upstream Markdown plugin pipeline remain partial; no row is promoted.

3. Command tails fold carriage returns, backspaces and bounded CSI erase-line,
   absolute column and relative horizontal cursor sequences across input chunks.
   Rewrites force replacement frames so previously emitted progress is corrected;
   rendered retention stays 64 KiB. This is not a full multi-line VT emulator.
   Installed Claude 2.1.316 was run in a private disk home with only Bash enabled,
   hooks/MCP disabled and no session persistence. A printf/sleep-2/printf command
   emitted tool_use at 1.97 s and both output lines in one tool_result at 4.00 s;
   partial stream frames carried tool input JSON, not command output. Consequently
   Claude command output remains completion-only on this observed protocol.

4. Bitbucket, Forgejo and GitCafe are advertised independently of CLI installation.
   Bitbucket bearer tokens (`BITBUCKET_ACCESS_TOKEN`) and Forgejo bearer tokens
   (`FORGEJO_TOKEN` plus explicit HTTPS `FORGEJO_URL`) have bounded, nonredirecting
   identity probes. 401 means unauthenticated; forbidden, offline, malformed and
   oversized responses mean unknown. No raw body, token or transport error is
   returned. GitCafe API validation and desktop credential storage remain
   unavailable and explicitly unknown; no authenticated-provider parity is claimed.
   The discovery RPC runs through the actual lib API reader; local HTTP failure
   fixtures verify classification/redaction, not live token acceptance by hosts.

5. GitLab status uses `glab mr list --source-branch ... --all --per-page 1
   --output json --order updated_at --sort desc`; opened/closed/merged metadata
   maps to the existing PR shape. An explicit `GITLAB_HOST` enables self-hosted
   identification, including ports. Local-only calls skip the host. Missing CLI,
   refused requests and invalid JSON retain null PR status. Background update
   checks and telemetry are disabled for this query.
   Opt-in `glab_live` ran real glab 1.126.0 (private binary, not on PATH; point
   `O3_LIVE_GLAB` at it) through the real server and lib VCS API against a private
   local GitLab API fixture: branch query, draft/open metadata, local-only and HTTP
   403 passed. Default tests use fake executables. Live authenticated GitLab and
   stale last-known-PR fallback/backoff remain unverified; the row stays partial.

6. Native icons follow the GTK3 configured theme, index.theme inheritance and
   declared directories (16 themes, 256 directories/theme, cycle/traversal guards),
   XDG data directories, hicolor and pixmaps fallback. Signed icon URL downloads
   exercise a cyclic inherited theme through the real server. Credentialed media
   selects image/audio/video extensions from content instead of always PNG;
   unknown content is rejected. Cache collection removes expired media after two
   grant lifetimes, examining at most 2048 entries per media creation. Tests retain
   fresh entries and remove expired entries. Full platform discovery, all media
   formats and MCP HTML injection remain partial; no row is promoted.


7. Fresh real T3 0.1.7-nightly.20261031.3991 was downloaded into a private disk
   profile, without accessing the installed T3 profile. The differential now
   executes actual lib projection/timeline/queue readers on captured output,
   asserts nonempty reader coverage and equality, and preserves boolean values
   and semantic counts instead of erasing them. Deferred workspace preparation
   now has its real turn item, stable provider linkage, unloaded session state,
   first-dispatch read watermark, local ordinal and thread metadata defaults.
   The paged-history regression verifies all 125 messages plus 125 preparation
   items survive snapshot/history paging. Progress folding also processes long
   rewritten lines with a mutable line buffer, avoiding per-character copies.

   [conformance-reader-report.json](conformance-reader-report.json): 92/119 whole
   surfaces match. subscribeThread snapshot/completion-marker timeline and queue
   views, including working state, activity/message/queue counts and fixture text,
   have zero sampled reader differences. Its only remaining captured wire
   difference is o3's internal executionRequested run field. Broader stream
   ordering, concurrent interleavings and native acknowledgement/recovery remain
   unproved; this sample does not promote subscribeThread or certify replacement.
   Earlier descriptions of the retained differential are historical.


Final verification for this follow-up: **189 default / 187 no-default all-target
tests passed**, with eleven explicit opt-ins ignored per configuration. Both
strict all-target Clippy configurations, formatting, diff checks and
`tools/gen-compat` passed. Real native Claude/Codex short turns passed projection
replay, timeline, queue, sidebar and awareness readers. Real glab 1.126.0 repeated its server/lib-reader test successfully. No compatibility row was promoted.

Release `tools/bench-rss` completed: **16,220 KiB (16.61 MB) HWM**,
below 50 MB, with 20 threads, 1000 messages, three subscribers and 1000 unary
calls. Provider registration/probes are included; inference and provider child
RSS are excluded. Aggregate evidence: [rss-reader-follow-up.json](rss-reader-follow-up.json).

### Fourth-audit resource bounds (2026-11-01)

Command folding consumes at most the last 256 KiB per chunk/final output, keeps
64 KiB tails, clamps cursor moves to the current line and ignores erase-before-cursor.
Producers run on blocking threads; folding holds neither the global tail map nor
the database writer mutex. Subscription cursors read the already folded output.
The adversarial cursor test has a two-second bound.

Search uses the reader connection on blocking RPC threads. Message/title scans
and thread rendering have byte/row budgets (8 MiB per phase, at most 4096 recent
rows plus 1000 FTS candidates); oversized records and older history may be omitted.
Results remain capped. GitHub media requires a registered canonical cwd and has
128 cache slots (10 MiB per file); a full cache refuses new media. Icon metadata
reads require regular files and at most 64 KiB, on blocking threads. Absolute
icons must remain within canonical XDG icons/pixmaps roots at mint and serve time.

Validation: 192 default / 190 no-default all-target tests, eleven opt-ins ignored
per configuration; strict Clippy both configurations, fmt and generated compatibility
docs. Release `tools/bench-rss`: **17,000 KiB (17.41 MB)
HWM**, excluding inference and provider child RSS; see [rss-fourth-audit.json](rss-fourth-audit.json).

## subscribeThread concurrency follow-up (2026-11-01)

The three o3-only run fields `executionRequested`, `recoveryPending` and
`recoveryReason` have no definition in the inspected T3 source and no reader in
lib-t3-rust. They now live in `run_internal`, separate from public `records`.
Startup migrates legacy run bodies, run-event payloads and cached launch
projections without changing event sequences. The migration regression verifies
boolean preservation, idempotence, public projection/replay/receipt exclusion and
recovery of an existing preparing execution. The existing two-process SIGKILL
regression still verifies native continuation and durable internal recovery reason.
Snapshots (including streamed/compact snapshots), getThreadProjection and history
read public records; event persistence excludes the private fields centrally.
There is no per-RPC sanitizer.

`tools/conformance-diff concurrent` adds a strict ignored test using the existing
fresh-profile/process-group harness and a credential-free Python `codex` peer on
each server's private PATH. Set `T3_REFERENCE_BIN` to a **server executable**, not
the npm downloader/launcher; the test requires nightly 3991's version. It implements
`--version`, initialize/initialized, account/read, model/list, skills/list,
thread/start/resume and gated turn/start/interrupt/completed. T3's generated Codex
schema requires full thread-start defaults (including approvalPolicy,
approvalsReviewer, sandbox, projectId and sessionId). No Claude fallback or real
inference was used. The source inspection covered CodexProvider.ts,
CodexAdapterV2.ts and effect-codex-app-server's generated response schemas.

Subscriber A attaches before the first message. Three separate connections queue
messages concurrently with a pin command while the fake native turn remains open.
Subscriber B attaches during execution. Interrupt with holdQueue, queue.resume,
release gates, quiescence and explicit user un-settle complete the scenario.
Both subscribers use the pinned real projection reducer and stream cursor reader;
actual timeline/queue readers run throughout. Each server verifies snapshot before
tail, increasing sequences, unique run creation and each of four user messages
exactly once. Both subscribers' final timeline/queue outputs equal fresh snapshot
reader outputs. Sidebar uses a nonempty authoritative shell snapshot, excluding
marked repository-enrichment metadata snapshots from replacement semantics.
This is reader-state equality, **not full JSON equality of every projection record**;
full reduced-record equality and exhaustive per-record causality remain unproved.

Three real reader bugs were fixed: queued user items now retain `queued_turn` intent
on subsequent updates, and Codex's session capabilities advertise the implemented
queued-message support, enabling the real queue reader's `canReorder`.
The legacy internal `attempt.updated` event alias is now persisted/replayed as
`run-attempt.updated`, which the actual projection reducer recognizes; a reducer
regression covers it and startup repairs historical aliases.
At that checkpoint the remaining strict differential failure was: T3 renders one
additional timeline activity group containing `run_interrupt_request` and
`run_interrupt_result`; o3 emits neither. This is client-read behavior, not an
accepted interleaving difference. O3's current local interruption/hand-off is not a
native execution acknowledgement (see RECOVERY.md); synthetic acknowledgement
records were not added to hide the missing lifecycle.

Final scenario runs 16, 17 and 18 (12.11 s, 11.67 s and 11.46 s) kept both
servers alive on distinct loopback ports and each reached quiescence on both servers and failed
on that same missing timeline group. No dropped/duplicated fixture messages or
nonmonotonic sequences were observed. Earlier harness iterations failed on the npm
launcher, incomplete command/peer schemas, treating reused checkpoint-scope
`.created` events as unique records, and treating marked shell metadata refreshes
as authoritative replacements; those are not server-equivalence evidence.
Normalization preserves semantic fields, booleans and counts. It erases opaque
revision/run identifiers and nonzero wall-clock instants, and sorts only the
independently dispatched queued-message rows and acknowledgement slots, preserving
the positions of the first message and causally dependent activities. The strict
comparison does not discard interrupt activities. No accepted-difference contract
was widened. **subscribeThread remains 🟡** for that concrete timeline gap and the
explicitly unproved full-record/native delivery/recovery behavior.

Harness iteration ledger (all failures are explicit, never silently accepted):

| Runs | Observation |
|---|---|
| 1 | npm launcher failed the pinned-version check; switched to the private server executable. |
| 2 | Incomplete command schema rejected with Missing key. |
| 3 | Fake thread/start response missing approvalPolicy; native turn never started. |
| 4 | Fake thread/start response missing projectId/sessionId; native turn never started. |
| 5–6 | Harness incorrectly rejected T3's reused checkpoint-scope.created ID. |
| 7 | Both scenarios completed; raw reader comparison exposed queue capability, queued intent and interrupt activity differences, plus expected timestamps/IDs/order. |
| 8, 10 | Harness incorrectly replaced the authoritative shell with a marked metadata refresh. |
| 9 | Explicit un-settle command lacked its required user reason. |
| 11–12 | Both scenarios completed; interrupt group mismatch and opaque sidebar revision difference remained. |
| 13–15 | Both scenarios completed; exactly one differing reader path: /thread/timeline/timeline (missing interrupt group). |
| 16–18 | Final paired-server harness repeated the same single reader path; per-server reader assertions passed each time. |

Final verification: **194 default / 192 no-default tests passed**, twelve explicit
opt-ins ignored in each configuration. Strict all-target Clippy passed in both
configurations; formatting, diff checks and `tools/gen-compat` passed. Initial full
suite attempts caught the hand-written test schema missing `run_internal`; the
fixture now uses the real schema initializer and both full suites were rerun.
The opt-in concurrent differential failed at this point on its reported client-read
gap; these totals do not count it as passed.

### Interrupt timeline pair, same day

The earlier claim that o3 showed both interrupt items at once described the
implementation before the independent audit below. It is superseded. The earlier
zero-difference result used an identity-erasing normalizer and did not check the
request-only window or exact replay; it is not evidence of current conformance.

## Independent interrupt audit implementation (2026-11-01)

No rows are promoted. The accepted-difference contract above is unchanged.

* F1/F2: confirmed. The deferred migration reproduces SQLITE_BUSY_SNAPSHOT after
  a second WAL writer commits. Migration now takes BEGIN IMMEDIATE before reading,
  persists progress in windows of at most 128 rows, and gates completed migrations
  with a schema marker. Legacy active execution bookkeeping, historical events and
  launch receipts remain covered by the migration regression. The bound is rows,
  not a wall-clock guarantee for an unusually large individual JSON receipt.
* F3: confirmed for native Interrupted. The supervisor now emits its result during
  terminal persistence, including unsolicited native interruption. Disabled restart
  continuation remains a separate server-local interruption (see RECOVERY.md).
* F4: confirmed. A live native turn now commits only the request before control
  handoff. Codex awaits turn/completed; Claude awaits its result boundary. A 15-second
  deadline fails the run with unknown native delivery, rather than claiming success.
  Process failure marks the control outbox delivery-unknown. Restart never resumes
  a run bearing an interrupt request. The differential gates the interrupt response
  and asserts working, canSteer and activeRunId during the request-only window.
  Exhaustive init/steer/death/deadline/restart race coverage remains open.
* F5: terminal mutation, missing root/provider ownership and missing pre-start
  attempts are rejected. Queued promotion now reserves the same ownership as an
  immediate start, fixing the queue regression without weakening validation. Full
  terminal-run background-work targeting and interruption of other provider owners
  remain open; these runs currently reject conservatively. The dispatch error tag
  was not differentially asserted against 3991. This is not full parity.
* F6: thread.stop now routes a live target through interrupt with holdQueue, or
  records stop intent and holds idle queued work. Its idle path is exercised against
  3991. Other-owner background work, idle-orphan cancellation and delegated completion
  cohorts and deleted-thread no-op edge parity remain open; the row stays partial.
* F7: the newer source's root provider-turn rule holds for the Claude result reducer.
  **Partially refuted against 3991:** gated Codex returns null for the result's
  providerTurnId; using the root turn produced two inspector-reader differences.
  Codex therefore preserves null on this pinned path. Claude live binary parity
  remains unproven; its root-turn relationship has a reducer regression.
* F8: confirmed in interrupt, message, command-output and both native item allocators. They now use
  max ordinal + 1; an imported sparse ordinal regression checks messages and signals.
* F9: reducer success, duplicate run creation, terminal-record causal order and
  exact replay sequence/identity are checked. Reader state in the request-only window
  is compared across servers. Run, source-item, provider-turn and merge-back IDs use
  one consistent mapping per server, preserving nulls and references. Revision hashes
  remain opaque revision values, separate from identities. Raw replay differences
  are reported and fail the strict test rather than being normalized away.

The strengthened replay check exposes a **reference-only** discrepancy: a live
thread event includes lastSnoozeWakeAt:null, but its replay omits that field.
The report retains the missing-versus-null distinction. This is outside the accepted
independent-operation interleaving exception, so the differential is not clean.
Idle-stop coverage also exposed missing latestRunStopRequested shell derivation;
o3 now derives it from the latest run's interrupt request, preserving sidebar readers.
See concurrent-reader-report.json for the latest strict comparison, including raw
replay discrepancies. These findings require further independent audit.

### Verification ledger

The first default/minimal full-suite attempts found two failing targets: background
(the peer blocked controls and retained an old terminal) and thread_commands
(queued promotion lacked ownership). Both were corrected and the targeted rerun
passed 13 tests with one live opt-in ignored. The first strict Clippy attempts
caught filter_map(bool::then) in the new test code; it was replaced with filter/map.

All differential attempts use the pinned 3991 executable and isolated homes:

| Attempt | Outcome |
|---|---|
| 1 | Failed: five reader paths; three revision hashes were mistakenly treated as identities, two result inspector turn IDs differed (reference null). |
| 2 | Compile failure in replay observer cursor type; corrected to u64. |
| 3–4 | Failed on newly asserted exact replay: reference live lastSnoozeWakeAt:null is absent in replay. |
| 5 | Compile failure in replay-difference collection; corrected. |
| 6–8 | Failed: reference replay discrepancy plus idle-stop sidebar pending/reason; missing stop-intent derivation was corrected. |
| 9–10 | Failed strictly: only the reported reference replay discrepancy remains. |
| 11–15 | Each completed both scenarios, then failed strictly on the same reference-only lastSnoozeWakeAt replay discrepancy. No other reader paths differed. |

Revision hashes retain their original opaque-value normalization; they are not run
or provider identities. Missing fields and nulls are never erased. The replay
comparison retains raw sequence/identity checks and reports other raw differences,
including reference defects, before failing. A reference-only defect is reported,
not accepted as a new contract exception or hidden by normalization.

Final validation: default cargo test --no-fail-fast passed 199 tests, with zero
failures and 12 opt-ins ignored; --no-default-features passed 197, with zero
failures and 12 ignored. The default suite was repeated after the late-handoff
outbox guard and passed again. Strict Clippy (-D warnings, --all-targets) passed
in both feature configurations. cargo fmt --check and tools/gen-compat passed.
Builds/tests ran serially with four Cargo jobs. No authenticated live-provider
or emulator verification was performed in this audit. Five final strict concurrent
repetitions (11–15) all failed only on the same explicitly reported reference replay
field discrepancy; none is counted as a passing differential.

The implementation is not ready for promotion: multi-owner background interruption,
complete thread.stop lifecycle behavior, the disabled-continuation terminal-state
choice against 3991, dispatch error-tag comparison and exhaustive init/steer/death/
restart race coverage remain open. Migration bounds rows per lock window rather
than guaranteeing a maximum duration for arbitrary historical JSON bodies.

## Bounded migration measurement (2026-11-02)

`events::migration_benchmark::large_profile_writer_windows` creates an isolated WAL
profile under `/var/tmp/o3-build`: 400 threads, 40,000 history events with 4 KiB
payloads, and 400 cached launch receipts with 1 MiB payloads. Measurements include
BEGIN IMMEDIATE through COMMIT (including lock acquisition), with synchronous=NORMAL,
a debug build and four Cargo jobs under nice; no concurrent build or test ran.
The original 128-row batches took 6,329.021 ms overall, with a worst measured writer
window of **1,389.677 ms** (324 windows). Adding a **2 MiB input-byte limit** per batch
reduced the worst measured window to **112.892 ms** (720 windows, 6,525.986 ms total).
The completed-marker reopen took 0.023 ms (baseline 0.027 ms).

Windows still contain at most 128 rows; a receipt larger than 2 MiB is processed
alone. Thus worst-case lock duration for arbitrary profiles remains unbounded by
time: one indivisible JSON record plus SQLite/WAL commit cost. The concrete measured
worst case above is workload evidence, not a universal latency guarantee. Migration
cursors and the completion marker retain their existing crash/resume semantics.
Reproduce with `cargo test --no-default-features --lib large_profile_writer_windows
-- --ignored --nocapture`, using the documented TMPDIR/CARGO_TARGET_DIR and nice/-j4.

## Control follow-up against nightly 3991 (2026-11-02)

This checkpoint supersedes the reference replay and disabled-continuation open
items in the preceding audit. No compatibility rows are promoted.

A. Replay remains a strict **per-server** invariant. Only for the reference, the
harness ignores a property path ending `/lastSnoozeWakeAt` when its live value is
null and replay omits it. Non-null values, the reverse direction, every other
field and all o3 replay differences still fail. A regression tests these negative
cases. `ignoredReferenceReplay` records the count separately from reader differences.
The accepted-difference contract at the stream-reader checkpoint is **not widened**:
this addresses reference self-inconsistency and changes no o3 behavior. A fresh
literal search across all three client repositories found zero matches for
`lastSnoozeWakeAt`. Replay metadata is excluded from cross-server comparison;
sequence, event identity and replay contents retain their own strict assertions.

B. **Open:** the run.interrupt implementation still rejects terminal-run background
work, and does not dispatch interrupts to other provider-thread owners. Existing
single-owner background tests do not establish either behavior against 3991.
The gated Codex control peer in this checkpoint does not synthesize a multi-owner
roster, so this remains an implementation and differential-evidence gap.

C. **Open:** idle stop/queue hold, missing-ownership fallback and deleted-thread
no-op are exercised against 3991. Idle orphan cancellation, delegated completion
cohort disposal and terminal/other-owner native background targeting are still
absent. These are concrete lifecycle gaps, not a demand for exhaustive scheduling.

D. Disabled continuation is **binary-observed**: kill an acknowledged running Codex
turn in an isolated profile with continueThreadsAfterServerUpdate=false, then restart
that profile. 3991 records cancelled with only the original user-message turn item;
no interrupt signals are invented. o3 now records cancelled, removes its artificial
error and cancels execution projections transactionally. The test compares normalized
real timeline/queue reader outputs strictly; the user's message must remain visible.
It waits for the persisted running/startedAt boundary before killing the server,
so native-peer receipt alone cannot stand in for durable execution acknowledgement.

E. Raw Effect frames now compare exactly (apart from substituting the generated
run id) for terminal, unknown, wrong-thread, missing-owner and deleted-thread
run.interrupt failures. The missing-owner fixture removes durable ownership from a
deferred run in each isolated database; it never changes a live profile. Missing
threads are tested for both commands. thread.stop succeeds for missing ownership
and deleted threads, so there is no rejection tag to equate in those cases.
o3 now preserves OrchestratorDispatchError and OrchestratorProjectionError cause
names, nested ProjectionStoreThreadNotFoundError and their messages/details.
The concurrent report retains both servers' sanitized raw exits.

F. Deterministic gated Codex tests exercise the four requested windows: native init
before acknowledgement (including release after cancellation and no late turn/start),
steer awaiting acknowledgement (working and no interrupt result until release),
process death after request, and restart after request before terminal acknowledgement.
o3's death/restart results are failed with delivery-unknown retained in SQLite and
no synthetic interrupt result. The reference instead reports interrupted after
death and cancelled after restart. Neither observed state proves native side effects
stopped. The existing recovery boundary is preserved. These are bounded tests for
these windows, not exhaustive scheduling or Claude transport proof. Pre-start
cancellation now queues adapter control and Codex checks it before turn/start.

G. The concrete lock measurement and byte-batch fix are recorded above. A single
arbitrarily large record remains indivisible; no hard duration guarantee is claimed.

### Follow-up execution ledger

Concurrent differential runs in this follow-up (separate numbering from the older
audit ledger):

| Runs | Outcome |
|---|---|
| 1 | Pass after the narrow reference replay exception. |
| 2–3 | Pass; raw rejection-frame discovery also exposed generic o3 cause names/messages. Raw frames were observed, not yet asserted. |
| 4 | Pass after matching cause names/messages and asserting raw Effect exits, including deleted-thread no-op. |
| 5 | Pass with missing ownership injected into each isolated deferred run. |
| 6, 7, 8, 9, 10 | Each passed: zero reader/replay differences, one ignored reference null-to-absent replay property each. This intermediate series overlapped preparation of the receipt-before-control cleanup; the final series verifies the stable code. |

The control-window differential was also run separately: attempts 1 and 2 failed
on a missing targetRunId in the new steer fixture (corrected); attempt 3 completed
the observation matrix and exposed disabled-continuation status divergence; attempt
4 failed the newly strict disabled reader check because the test killed o3 before
its peer acknowledgement had reached the durable run. The fixture now waits for
running plus non-null startedAt. Attempts 5 and 6 passed, including strict disabled
reader equality. Attempt 6 additionally asserts no late native turn after init
cancellation. Final verification repeats these checks after moving pre-start
control delivery after the committed receipt.

B and C remain open for the concrete implementation gaps above. No live provider,
Claude race matrix, side-effect cancellation proof or exhaustive schedule claim
follows from these credential-free Codex peers. Compatibility row statuses stay
unchanged pending independent audit.

Final concurrent runs **11, 12, 13, 14 and 15 each passed** against pinned 3991 on
the stable code: zero reader differences, zero remaining per-server replay
differences and exactly one ignored reference null-to-absent property per run.
`O3_CONCURRENT_REPORT` regenerated [concurrent-reader-report.json](concurrent-reader-report.json)
on each run; it contains run 15 and both raw Effect exit sets. The final
control-window attempt **7 passed** after receipt-before-control cleanup, including
strict disabled reader equality and the no-late-turn init assertion. Sanitized
observations are in [control-window-report.json](control-window-report.json).

Final required checks: default `cargo test --no-fail-fast` passed **200**, with
**0 failed / 14 ignored**; `--no-default-features --no-fail-fast` passed **198**,
with **0 failed / 14 ignored**. The opt-in binary differentials and migration
benchmark were executed separately as recorded above. Strict all-target Clippy
passed in both configurations, as did `cargo fmt --check`, `tools/gen-compat` and
`git diff --check`. The first Clippy attempt found the new RPC helper after a test
module; moving the unchanged helper before dispatch resolved that lint. All Cargo
builds/tests ran sequentially under nice with four jobs. No rows were promoted.


### Implementation audit follow-up

`thread.stop` now targets waiting native runs. The pinned 3991 approval probe
stops the run but retains the unresolved pending/live runtime request while the
native session stays open, even after serverRequest/resolved; channel loss during
fixture cleanup cancels it. o3 follows that observed boundary rather than inventing
a native approval decision. Both subscribers compare the real approval reader
and decision builder while the request is pending. The compaction candidate reader
also participates in convergence; the gated peer emits no context/token usage,
so this does not establish an actionable compaction candidate.

The attempted background-after-result concurrent extension did not establish the
required reference state: 3991 completed run 6 after turn/completed despite an
in-progress commandExecution item, and exposed no pendingBackgroundTasks roster.
The extension's wait for retained ownership failed. Its fixture mode remains for
further investigation; the stable concurrent scenario includes approval-pending
stop. Claude and Codex adapter tests separately prove interruption after a known
terminal with owned tasks/items and no second terminal result. No reference
background targeting equivalence is claimed.

The terminal reopen test now drains the restarted shell to exit before clear.
The repository identity test waits for a resolution event with its roots metadata;
a current snapshot may already contain the new identity but lacks that metadata.

The turn ordinal expression index retains sparse-import correctness and is checked
with EXPLAIN QUERY PLAN. Stop-request presence uses the deterministic item primary
key. Quiet provider supervisors use change notifications; deadline tasks exit on
terminal state changes instead of retaining State until timeout. See RECOVERY.md
for the overall 75-second control budget and its uncertainty boundary.


The enlarged scenario exceeded 3991's replay cap (138 events caused a snapshot
fallback). The harness now verifies two complete replay ranges separated by a
checkpoint before the approval scenario, checks event counts, and bounds replay
waiting. It does not discard the prefix. The approval reader exposed two actual
Codex projection differences: reason precedes command in the prompt, and a pending
approval item stays waiting after its run stops. Both now match the pinned binary.
Server-derived request IDs and timestamp strings are normalized consistently;
request capability, choices, detail text and status remain strict.


Negative controls removed the waiting target fix and each adapter's retained
terminal boundary: the real Connection stop test timed out, and the two separate
single-terminal background tests both failed. The initial end-to-end Claude init
probe could pass without its guard because process reaping won the pipe-read race.
A direct adapter test now keeps the output receiver alive and gates initialization;
it deterministically fails on the forbidden Started event with the guard removed.
It covers both watch cancellation and queued Interrupt before initialization ends.
The four end-to-end Claude windows remain covered separately.

Concurrent execution ledger for this implementation audit (independent numbering):

| Run | Result |
|---|---|
| 1 | Failed: the approval peer omitted required request fields, so 3991 never projected a request. |
| 2 | Failed: incorrectly expected the pending approval to close immediately with the run. |
| 3 | Failed: the same expectation after adding serverRequest/resolved. |
| 4 | Failed: waiting for request closure timed out; SQLite showed closure only during server cleanup. |
| 5 | Failed: numeric RPC request IDs did not change that reference boundary. |
| 6 | Failed: reference completed the background fixture's turn without retaining ownership. |
| 7 | Stopped after replay waited indefinitely for 138 events; the reference had returned a snapshot. The isolated orphan process group was cleaned up. |
| 8 | Failed: the new bounded replay assertion confirmed one snapshot versus 138 expected events. |
| 9 | Did not compile: helper extraction compared Value with &Value; corrected by dereferencing. |
| 10 | Failed: seven new approval reader differences (timestamps/derived IDs, reason text and pending item status). |
| 11 | Passed: zero reader/replay differences. |
| 12 | Passed: zero reader/replay differences. |
| 13 | Passed: zero reader/replay differences. |
| 14 | Passed: zero reader/replay differences. |
| 15 | Passed: zero reader/replay differences. |

Runs 11–15 each recorded exactly one narrow reference null-to-absent replay
exception. These are five consecutive passes against the pinned versioned binary.
The full control-window matrix also passed all 14 observations (10 Codex across
both servers, four Claude on o3), including the 20-second steer hold. Its sanitized
results replace control-window-report.json. An initial full suite had one failing
approval assertion caused by the incorrect immediate-cancellation expectation;
that expectation was corrected using pinned binary evidence.

The first no-default-features full suite exposed a test synchronization race in
the new approval stop test: a request can be visible before its run reaches
waiting. The fixture now waits for both predicates before dispatching stop;
production code was unchanged. All three required full suites were repeated.

A later repeat found another pre-existing fixture race: vcs_compat replaced the
process-wide fake gh executable while the concurrent stacked-action test created
a PR, returning JSON where a URL was expected. The fake pr-list response is now
workspace-scoped; no executable replacement occurs. The seven VCS tests passed
with four test threads before repeating the full required verification.

Final stable concurrent runs **16, 17, 18, 19 and 20 each passed** with zero
reader differences, zero replay differences and exactly one recorded reference
replay exception. O3_CONCURRENT_REPORT regenerated concurrent-reader-report.json
on each run; the committed report is run 20 and describes the expanded readers
and both complete replay checkpoints.

Final required verification after fixture synchronization fixes: default full
suite **206 passed / 0 failed / 14 ignored**, twice; no-default-features full
suite **204 passed / 0 failed / 14 ignored**. Strict all-target Clippy passed in
both configurations, together with cargo fmt --check and tools/gen-compat.
Compatibility statuses are unchanged from b05b511; only the requested notes changed. All builds and tests ran sequentially under
nice with four Cargo jobs; full suites used four test threads.


Release comparison with tools/bench-rss on two verified binaries: an archive of main
(1eb59d7, sha256 `9cff9a34`) and this branch (sha256 `cccc0b13`), with three runs
each, alternating main and branch. An earlier table here compared main with itself:
both reports carried the main hash, and its reported speedup is withdrawn. The workload creates 20 threads,
seeds one with 1,000 deferred messages of about 1 KiB, measures the last 100 message
dispatches, then appends 100 thread-metadata updates on that long thread; three bounded
subscribers and 1,000 settings reads exercise RSS retention. Latency is loopback
end-to-end RPC wall time including event append, receipt and shell projection.

| Measurement, range of three runs | main | branch |
|---|---:|---:|
| Message dispatch median (ms) | 2.430 to 2.513 | 2.551 to 2.616 |
| Message dispatch p95 (ms) | 3.014 to 4.000 | 2.768 to 3.291 |
| Metadata append median (ms) | 0.427 to 0.461 | 0.448 to 0.465 |
| Metadata append p95 (ms) | 0.483 to 0.613 | 0.611 to 0.957 |
| Peak RSS across phases (KiB) | 16508 to 16864 | 15964 to 17428 |

The branch costs about 0.1 ms more per message and up to 0.03 ms per event; peak RSS
ranges overlap, with the highest branch run about 0.6 MB above the highest main run.
All stay far below the 50 MB target. Provider inference and child-process
memory are excluded. Raw sanitized runs and hashes:
[rss-control-audit-main.json](rss-control-audit-main.json) and
[rss-control-audit-branch.json](rss-control-audit-branch.json).

Items 1–6 and 8 are addressed. Item 7 adds strict pending approval/decision-reader
convergence and the compaction candidate reader; actionable compaction and a
reference background-after-result ownership fixture remain unestablished as
explained above. No compatibility status was promoted.

### Requests after a stop

The final audit reproduced Approve failing with `provider request is no longer live`:
after a stop the provider channel was gone while the request still advertised `live`.
Closing the request at run end was tried and rejected: the differential showed 3991
keeps it pending and live until it releases the session, which sidebar, approvals and
compaction readers all show, so that would break interchangeability.

O3 now matches that window. A live request of a finished run stays offered for T3's
30-minute session idle time; an answer inside it is recorded as resolved with nothing
delivered, because the native turn already ended, and adds no control outbox entry. After
the window it becomes `expired` with a `not_resumable` capability and its turn items
`cancelled`, as T3 does for a released session. `tests/session_approvals.rs` drives both
outcomes through the real approvals reader and command builder, including an answer while
the supervisor still holds the closed channel. Observed on 3991: the request stays pending
and live at the differential's checkpoints. From source only: the 30-minute default, the
expiry at release and the late answer, because the differential builds that answer without
dispatching it and does not wait out the window.

## P0 promotion after five audits (2026-11-02)

`orchestration.subscribeThread`, `orchestration.dispatchCommand`, `run.interrupt`,
`thread.stop`, `runtime-request.respond` and `message.dispatch` are promoted after the
fifth independent audit, of head 9a207e6, found nothing at any severity and gave all six
a pass under the reader rule. It ran the full suite (207 passed, 0 failed), the pinned
concurrent differential (zero reader and replay differences, one reference exemption) and
two private tests of late question answers and dismissal through real readers.

Not established, and stated in the row notes: exhaustive race schedules, authenticated
live provider inference for these paths, and the 30-minute expiry or late answer as
observed on 3991, where they come from source. `orchestration.subscribeShell` and
`server.getConfig` stay partial for their own reasons.

The merged release also started twice on a private copy of a real imported profile (375
threads, 25 projects, 800 MB database), which ran the migration for real: thread and
message counts kept, none of the moved run fields on the wire, SQLite integrity check ok,
14.5 MB RSS after startup, clean exit both times. Start time was not measured.

## P1 consumed-reader implementation (2026-11-02)

The pre-edit inventory and per-row opinions are in
[p1-reader-inventory.md](p1-reader-inventory.md). No compatibility status changes.
`subscribeShell` and `getConfig` were excluded. No live inference or credentials
were used; the reference is the isolated versioned 3991 binary, not newer source.

Two fixes:

- Default search no longer synthesizes message matches from thread titles.
  The real lib `search_threads` reader on 3991 returned zero matches for threads
  whose unrelated messages did not contain the title query; o3 returned title
  snippets. Desktop `src/thread_search.rs:39` and Android's MessageSearch consume
  that array. [Before](p1-reader-before.json) preserves the counterexample;
  [final isolated report](p1-reader-report.json) preserves ordered matches and
  actual downloaded attachment bytes, without scalar-shape normalization.
  `tests/threads_compat.rs::default_search_does_not_report_title_only_hits_as_messages`
  uses the actual lib API, checks that only a matching message appears, and
  retains explicit title-search coverage. Full message ranking is not certified.
- Explicit idle instance switching resets foreign native session IDs, separates
  provider process generations and sends a bounded transcript to the fresh
  session. `tests/model_handoff.rs` drives actual Claude/Codex adapters with the
  deterministic `tests/fixtures/input/handoff.py` peer through a Connection, then
  applies the real timeline reader. Both directions retain user/assistant text;
  the peer rejects foreign resume/fork calls. Removing the fix fails the test.
  Attachment/context parity is not claimed. No production literal caller of the
  explicit command was found beyond the lib allowlist; desktop `src/write.rs:189`
  instead supplies a per-message selection. That path is not certified by this
  explicit-command test.

Additional evidence: native Claude structured-question dismissal now goes from
`questions::command` through the native can_use_tool response to a peer asserting
`deny`, then through `questions::pending` to the cancelled/hidden request and
completed run. It adds coverage, not a new production recovery implementation.
`tests/command_output.rs` applies real subscription frames to the shared
`parity::output` reader, in addition to desktop-style assembly.

Consumer correction: current desktop find (`ui/qml/T3Panel.qml:193`) and Android
find (`ui/thread/TimelineReader.kt:87`) search loaded rows locally. No production
`orchestration.searchThread` RPC consumer or actual lib RPC reader was found.
Its historic Markdown segmentation limits are not a demonstrated consumed gap.
Under the strict rule this is an audit/classification question, not a green-row
claim based on a fabricated reader.

Remaining consumed/unproven work, by row: broader media adapter/format evidence;
message search ranking and fallback bounds; command multiline VT/reference live
output; GitCafe/desktop source-control auth; non-GitHub/GitLab status and stale PR
fallback; historical fork attachment/context continuity; cross-provider native
context and per-message selection; dismissal restart/delivery-unknown windows.
No opaque native-resume implementation equality or exhaustive schedule equality
is demanded. The accepted independent-record stream contract does not excuse
missing client-visible history or state.

For config/settings, external CLI updates of unknown ownership and the agent
browser service are owner decisions. Startup/custom discovery, generic model
option/config normalization and settings effect coverage, and tool-free Codex
text generation are separate non-owner gaps. Existing delta/native/scoped reader
coverage remains evidence for its tested subset only. Neither owner decision is
used to waive these remaining gaps.

The source-control and settings limits are concrete: `src/vcs.rs:961` explicitly
reports GitCafe credential validation unavailable; desktop `src/write.rs:75`
consumes its auth.status. `src/title.rs:67` rejects Codex text-generation selection,
which the native settings reader exposes (`lib/src/native_settings.rs:56`). These
remain separate from the two owner decisions. Search regression also fails under
a negative control restoring unconditional title hits.

The isolated P1 comparison initially rejected incomplete thread-create arguments
and workspace-file previews (preview type/path validation); it was narrowed to a
real uploaded attachment. Those rejected probes are not asset parity evidence.
The final assertion compares actual bytes and ordered search match values. The
harness now defaults directly to the pinned versioned binary; it no longer picks
the newest installed runtime by enumerating versions.

Further asset-consumer correction: no production caller in the three trees sets
renderIntent:mcp-app. Desktop src/documents.rs:10 fetches ordinary inline HTML
attachments and ui/qml/T3Backend.qml:1202 supplies a local MCP bridge. The optional
lib API descriptor is not a production caller. Therefore server HTML injection
is not an established consumed asset gap. Media still has real consumers at
desktop src/videos.rs:28 and Android core/transport/NativeT3Transport.kt:70; this
run proves only the reported attachment download, not every media adapter.

Final verification on the implementation commits: default `cargo test
--no-fail-fast` **209 passed / 0 failed / 15 ignored**;
`--no-default-features` **207 passed / 0 failed / 15 ignored**. Both strict
all-target Clippy configurations, fmt check, gen-compat and diff check passed.
The added isolated P1 differential passed against 3991 after the search fix;
the concurrent differential ran once and passed its zero reader/replay difference
assertions under the existing reference-only replay contract. Earlier local P1
probe failures described above are not counted as passing parity evidence.

`tools/bench-rss` rebuilt the release binary and passed: **16,248 KiB / 16.64 MB
HWM**, below 50 MB. Message RPC last-100 median/p95: **2.709 / 2.967 ms**;
metadata RPC median/p95: **0.463 / 0.665 ms**. This is a single workload sample,
not a baseline speedup claim. Search removes the default title-scan pass; the
transcript query is limited to explicit idle instance changes, not ordinary
append/subscription paths. No background hot-path task was added. Raw sanitized
sample and binary hash: [rss-p1-reader.json](rss-p1-reader.json). Inference and
provider-child memory are excluded. All Cargo work ran serially under nice with
four jobs; full suites used four test threads. No emulators, services, deployment,
pushes or compatibility promotions were performed.

## P1 audit corrections (2026-11-02)

The independent audit of the P1 block approved its two behavior changes and corrected
the inventory in [p1-reader-inventory.md](p1-reader-inventory.md), which is kept as the
author's starting point. Where they differ, this section wins.

- `orchestration.searchThread` and `thread.model-selection.set` have no production caller
  and move to unused. `thread.user-input.dismiss` and `subscribeServerConfig` are promoted.
- The native dismissal test is transport evidence: the client UIs do not offer dismissal
  for its live Claude request. Comparing ordered empty search results is not ranking
  evidence. Unary settings readers do not show a gap in the subscription.
- Remaining client-visible gaps, by impact: historical forks losing attachment context
  (`src/thread_commands.rs`); per-message provider switching through `message.dispatch`;
  message-search snippets that omit the match (`src/threadq.rs`); workspace `.aac`, `.opus`,
  `.avi` and `.ogv` rejected (`src/assets.rs`); PR metadata lost on a transient failure;
  multi-line command-output redraws; Codex text generation; GitCafe credential status.

## Client-visible audit implementation (2026-11-02)

No compatibility status is promoted. Evidence here applies only to the tested
reader paths; independent audit decides row status.

Codex text generation remains unavailable. An offline probe of installed CLI
0.166.0 generated both stable and experimental app-server schemas under an empty
isolated home, without starting a thread or inference. Neither thread/start,
thread/resume nor turn/start exposes a tool policy, tool choice or universal
empty tool allowlist. The property lists and schema hashes are recorded in
[codex-tool-free-probe.json](codex-tool-free-probe.json). The config object accepts
arbitrary keys, so accepting an invented disable flag would not establish that
it takes effect. Read-only sandboxing permits reads and commands and does not
meet the requested tool-free guarantee. The adapter therefore still rejects
text-only mode rather than treating a prompt or sandbox as a tool ban.
[Official app-server documentation](https://learn.chatgpt.com/docs/app-server)
describes dynamicTools as additional client tools, not a switch for built-ins;
the [configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
provides individual feature controls, not a verified universal tool-free mode.
This is a limitation of the inspected installed contract, not proof that every
future version lacks such a mode. No live inference was attempted.

Implementation evidence for the remaining audit points:

1. Historical forks: `tests/model_handoff.rs` uploads an image, completes two
   native turns, forks the first and requests the image again. Before the fix,
   the deterministic peer rejected the fresh prompt and the run failed while
   the projection still contained the inherited attachment. Afterward both
   Claude and Codex peers verify actual PNG bytes, historical context and the
   absence of the later turn; the real timeline reader reads the response.
   Descriptors are deduplicated and limited to 32 / 64 KiB; transcript context
   is limited to 1 MiB, and aggregate image materialization has an 8 MiB budget.
   Oversize or missing content fails explicitly instead of silently dropping it.
   New fresh sessions use the same capture for rollback and provider switches.
2. Per-message selection: the previous implementation fails the new dispatch
   variant of the bidirectional native-peer handoff test. The corrected path
   shares selection validation, foreign identity reset and transcript capture
   with explicit selection. A same-instance model change retains the generation
   and resumes existing native history. Android also sends this selection:
   `ThreadViewModel.kt` passes it to `MessageSender.messageCommand`, whose
   `modelSelection` accompanies `message.dispatch`. No client changes required.
3. Search: the expanded P1 differential failed on both snippet contents and
   order. The pinned binary places 24 characters before these long matches,
   with a 238-character body and boundary ellipses. Both positive and negative
   queries now compare ordered actual lib reader results successfully, including
   substring hits. The local nightly source uses a different window (72 / 236)
   and is source-only evidence, not the authority for this change. The separate
   UTF-8/folded-accent regression finds CAFÉ after 350 non-ASCII characters.
   Message choice prefers user over assistant, then message recency and ID;
   result order prefers user, then thread recency and ID. Existing scan budgets
   remain; this is not exhaustive ranking certification.
4. Media: the real `asset_bytes` reader initially received
   `AssetPreviewTypeValidationError`. It now downloads exact workspace fixture
   bytes for AAC, Opus, AVI and OGV with desktop classifier MIME mappings.
   Existing path, scope, signed identity and content-selected cache checks are
   retained. The new fixtures test byte transport, not codec playback.
5. PR status: the real-reader fixture initially lost PR 7 on a simulated CLI
   failure. It now retains it for the same identity and proves separation on
   branch and remote changes, authoritative empty-result invalidation and
   `localOnly`. Cache entries are bounded to 128 successful identities and five
   minutes, keyed by canonical repository identity, canonical workspace root
   and branch. No additional host request or background refresh is introduced.
6. Command output: the failing two-row redraw now folds split CSI A/B controls
   across chunks. Actual live and settled server frames pass through lib
   `parity::output`; the reader receives the rewritten two rows. A/B/E/F motion
   clamps to retained rows. Existing 256 KiB per-input/final and 64 KiB tail
   bounds remain, plus an 8192-row temporary bound. The adversarial large-move
   test asserts a two-second ceiling and bounded output; its debug test group
   completed in 0.03 seconds on the focused run. This is bounded folding, not
   full VT terminal emulation.
8. GitCafe: [official REST documentation](https://git.cafe/docs/reference/api)
   explicitly documents bearer authentication at `/api/auth/principal`.
   Its [OpenAPI contract](https://git.cafe/api/openapi/json) specifies `actorId`
   and `handle`. The isolated fixture initially returned unknown; it now
   authenticates that schema. Only `CAFE_TOKEN` against the fixed HTTPS endpoint
   is supported. Five-second timeout, 64 KiB response cap, no redirects and
   generic errors remain; responses and credentials are not exposed. No real
   credentials or authenticated live endpoint were used. Successful GitCafe
   discovery has not been driven through the complete Connection reader, so
   this remains partial evidence and is not a green-row claim.

Final validation: default `cargo test --no-fail-fast` **214 passed / 0 failed /
15 ignored**; `--no-default-features` **212 passed / 0 failed / 15 ignored**.
Both strict all-target Clippy configurations (`-D warnings`), `cargo fmt --check`,
`tools/gen-compat` and diff check passed. The final pinned P1 and concurrent
isolated differentials each passed; P1 now compares nonempty ordered message
matches. The native fixture was additionally tightened to require a prior seed
or the correct native resume on same-instance model changes; focused tests pass
in both feature configurations. An initial tightening incorrectly treated the
seed plus appended user context as a different seed; correcting the fixture's
prefix recognition resolved that test-only failure.

`tools/bench-rss` passed with **17,556 KiB / 17.98 MB HWM**, below 50 MB.
Last-100 message RPC median/p95: **2.758 / 3.121 ms**; metadata median/p95:
**0.475 / 0.687 ms**. A second invocation with
`O3_BENCH_MODEL_SELECTION=1` measures the normal client path carrying an unchanged
selection on every message: **16,972 KiB / 17.38 MB HWM**, message median/p95
**2.635 / 3.172 ms**, metadata **0.444 / 0.540 ms**. These are single samples,
not evidence of a speedup or a statistically established zero cost. The previous
recorded message sample was 2.709 / 2.967 ms; workload variation is not isolated.
The unchanged-selection path returns before transcript capture or extra database
queries. Historical capture occurs at a fork, rollback or instance change, not
on ordinary appends. Multiline parsing is bounded by the retained tail plus the
per-chunk limit and runs outside the hub's global lock.
Sanitized measurements and release binary hashes:
[rss-client-gaps.json](rss-client-gaps.json),
[rss-selection-client-gaps.json](rss-selection-client-gaps.json).
Inference and provider-child RSS are excluded. All builds and tests ran serially
under nice with four Cargo jobs and four test threads; no emulator, deployment,
service change, push or merge was performed. Compatibility statuses are unchanged.

## Independent audit follow-up (2026-11-02)

This section supersedes remaining-gap rationales in earlier inventories.
Compatibility statuses are unchanged; independent audit decides them. Each
reported defect was reproduced before its fix using an actual Connection.
No credentials, inference, services, emulators, pushes or merges were used.

1. Repository endpoint identity: the PR fixture cached PR 7 at port 8443,
   changed the remote to 9443 and simulated host failure. Before the fix it
   returned the foreign PR; after the fix it returns no PR. The actual lib
   git_controls validator/actions reader also failed with the PR action disabled
   before the fix and passes afterward in both feature configurations. Same-endpoint,
   branch-change and authoritative-empty tests still pass. Nondefault ports
   are retained in canonicalKey, also used by desktop connection routing and
   lib sidebar grouping. Correction below: SSH port 22 did not normalize in this revision; ordinary
   URL/SCP repository grouping is retained. Project IDs, roots and receipts
   are unchanged. Android stores repositoryIdentity as shell metadata.
   Provider classification uses hostname; requests/cache retain the endpoint.
2. Native continuity: a latest-completed native fork with 33 attachments
   previously returned historical attachments exceed handoff budget; it now
   reaches native continuity without fallback capture. An older boundary with
   the same attachments still fails explicitly and creates no fork. Client
   fork actions propagate RPC failures; neither renders attachment-omission
   warnings from providerHandoff. Attachment overflow therefore remains a
   rejection rather than dropping images the inherited timeline displays.
   Within bounds, historical attachment continuity works. Pinned overflow
   behavior was not driven here. The newer local ContextHandoffService source
   truncates imported text and records coverage: source-only evidence.
3. Tool results: the historical-fork peer previously failed because its fresh
   prompt omitted tool-output-proof. Both adapters now supply stored tool
   input/output/results alongside messages up to the chosen run, excluding
   context-discarded/later runs. The real timeline reader still receives the
   completed response. The fixture inserts a normalized stored Read result;
   it does not claim new real-provider tool-execution evidence. A separate
   bounded-context test retains the newest result, drops oldest chunks and
   includes an explicit truncation marker within 1 MiB.
4. Search: the real search_threads reader previously returned no match for
   suffixneedle after 70,000 characters. Exact-token FTS and substring fallback
   now find it in a centered snippet. Each phase retains its 8 MiB byte scan
   budget and candidate-count bounds. Candidates beyond that byte bound remain
   outside coverage. The existing adversarial command-output time/size test
   is unchanged. Exhaustive ranking is not certified.
5. Generation: settings saves remain accepted. Desktop native_settings::execute
   propagates rejected updates before refreshing fields; T3Backend.qml exposes
   settingsResult.error. Rejecting the save would make a valid selection
   unusable. Explicit title regeneration (including generateTitle launch),
   generated blank commits and reviews now return an actionable instruction
   to choose a Claude selection or supply text manually when Codex is selected.
   Explicit preferences no longer silently choose the conversation provider.
   The settings/title Connection regression failed before and passes after;
   existing title tests now expect this error. Automatic title derivation
   without an explicit generation request can still use deterministic text
   fallback. Tool-capable generation is never started.
6. Shell: the materialized helper previously returned zero active threads from
   10,001 live, one old archived and 10,001 newer deleted synthetic records.
   It now filters deleted rows in SQL and includes all live/archived threads,
   as streamed snapshots already did. Projects also have no arbitrary 10,000
   materialization limit. Actual Connection HTTP shell/archived readers retain
   these records; complete snapshots need no settled-older recovery flag.
   The scale benchmark additionally checks subscribeShell and reports release
   server-only RSS separately from combined debug integration-process memory.

Attachment continuity within bounds is established by deterministic peers and
actual readers. Unproven codecs, complete VT emulation or live authentication
alone are not consumed gaps. Other-host PR metadata, broader search ranking and
settings effects, and desktop credential storage still need specific consumer
and reference evidence. No row status was promoted.

Final validation: default suite **219 passed / 0 failed / 16 ignored**;
no-default-features **217 passed / 0 failed / 16 ignored**. Strict all-target
Clippy passes in both configurations; fmt, gen-compat and diff checks pass.
Pinned 3991 P1 and concurrent differentials pass. The separate pinned settings
probe confirms acceptance and retention of the Codex selection through the real
config reader: [settings probe](generation-settings-reference.json). Its first
attempt used server.getSettings, which 3991 does not expose; the corrected probe
uses server.getConfig. An initial full suite failed one old unit expectation
for silent provider substitution; the updated expectation and full rerun pass.
Clippy initially rejected helper placement after the test module; moving those
helpers before the module resolves it without changing behavior.

The normal release benchmark passes: **17,060 KiB / 17.47 MB HWM**, below 50 MB;
last-100 message median/p95 **2.648 / 3.078 ms**, metadata **0.449 / 0.708 ms**.
[Normal release measurement](rss-audit-followup.json). The separate 20,003-record
synthetic release profile returns all **10,001 active + one archived** thread:
subscribeShell **852.27 ms**, archived-shell RPC **882.77 ms**. Server-only HWM
is **15,172 KiB / 15.54 MB** after streaming and **95,740 KiB / 98.04 MB** after
the materialized archived-shell call. The large-profile materialization therefore
exceeds 50 MB; the normal benchmark remains below its target. This is a measured
cost of materializing all shell rows, not a constant-memory claim. Client and
provider-child RSS are excluded. [Scale measurement](rss-shell-scale.json),
reproducible with tools/bench-shell-scale. Both samples use the same recorded
release binary hash. They are individual workload samples, not speedup claims.
All Cargo work used nice and four jobs, one build/test at a time, with four test
threads in the full suites. Compatibility statuses are unchanged.

## Client-visible automatic-flow and scale audit — 2026-11-02

Statuses are unchanged. This section supersedes the earlier claims that automatic
launch/commit generation should reject unsupported preferences, that every default
port already normalized, and that historical attachment overflow should reject.

1. **Automatic generation:** the real title/stacked-action readers failed first
   with a saved Codex preference and now pass. Android's automatic unnamed chat
   launches successfully: its title becomes the normalized first 80 characters of
   the initial message. Blank-message commits show `Update <file>` or
   `Update N files`, and finish normally. The saved preference remains intact;
   no other provider is selected. Explicit regeneration/review without fallback
   still returns the actionable unsupported-generation error. Pinned 3991 accepts
   the settings save and an empty automatic launch (`shell-preview-reference.json`);
   its generation with an initial message remains unproven without inference.
2. **Shell size:** a real HTTP Connection with 375 long latest messages failed
   `too_large` before the fix. Afterward HTTP and real shell stream readers pass.
   Pinned 3991 returns `latestVisibleMessage:null` for the deferred long-message
   fixture; newer ProjectionStore source also emits null. No consumers of this
   field were found in the checked Rust/Android sources: sidebar/awareness use
   title, status and activity stamps; search uses separate results and snippets.
   Shell summaries now emit null, including incremental `thread.updated` events,
   without reading/duplicating complete latest message bodies. Thread projections
   retain the full content. This is the sampled V2 behavior, not a claim about
   every possible reference run state.

   The 375-row fixture is 280,852 bytes (about 749 bytes/row). At this row size,
   10,001 rows need roughly 7.5 MB plus envelope/projects and fit 8 MiB; 12,000
   need about 9 MB and cannot. These are estimates, not bounds for arbitrary
   metadata: long titles, projects, task arrays and inherited fields can also
   exceed the limit. A single incremental row has the same null summary, so a
   long message alone cannot make it oversized; arbitrary metadata still can.
   There is no compatible pagination for shell snapshots in these clients.
   All live threads are retained. Above the client limit, HTTP reports too_large
   and streams disconnect, including reconnect; no scalable-load guarantee is
   claimed. No threads are silently dropped to conceal that limitation.
3. **Archives:** a regression with an invalid active row failed first because
   archive reads tried to build it; it now passes with an empty archive. SQL
   selects archived nondeleted rows before constructing summaries. Message text
   is not materialized. Existing desktop/Android archive readers request the
   entire archive; no compatible cursor/limit contract was found. The archive
   itself can still exceed the frame limit for sufficiently many archived rows.
   Release measurements below exclude client/build/test-process RSS.
4. **Repository identity:** SSH `:22` failed first and now passes with the SCP
   key; HTTPS443/HTTP80 also normalize. Nondefault ports remain. HTTPS and SSH
   at the same nondefault port intentionally share the host:port/path key.
   The checked lib sidebar reads `canonicalKey` (or `origin.canonicalKey`), not
   the remote URL; it does not independently compute default ports or schemes.
   Desktop routing uses that supplied key. Existing cross-repository PR cache
   regression remains in the suite.
5. **Search:** `cafe`, `afe` and a decomposed query failed before and now find
   `CÁFE`; late snippets preserve the original accents and include the match
   despite hundreds of preceding combining marks. Folding removes zero-output
   marks; FTS terms use the same fold and snippets map offsets back to source.
   **No wall-clock bound exists** for adversarial FTS queries. Byte/candidate
   budgets do not bound SQLite ranking before LIMIT; no deadline is claimed.
6. **Historical forks:** the older-boundary 33-attachment fork failed first;
   it now succeeds, keeps the newest 32 within 64 KiB, and places
   `[Historical attachments truncated: oldest descriptors omitted; newest retained.]`
   in inherited context delivered to the agent. Oversized descriptors are omitted
   within the same budget. Native forks retain their native path. The inherited
   timeline still shows history; the marker is agent context, not a client toast.
   Pinned overflow behavior and a fresh native peer observing this marker were
   not exercised; reference parity is not certified.
7. **GitCafe CLI login:** the fixture's real discovery Connection first reported
   unauthenticated without CAFE_TOKEN. It now reports authenticated and the
   fixture handle through documented `cafe api /auth/principal`, fixed to
   https://git.cafe/api with noninteractive flags, 5 s and 64 KiB limits. The CLI
   handles its login; the server never retrieves a token. Failure/malformed
   output remains unknown. Desktop readiness excludes only `unauthenticated`; the
   separate clone-tracking capability gates that flow.
   No real credentials or real authenticated requests were used. Documentation:
   [CLI](https://git.cafe/docs/reference/cli),
   [API](https://git.cafe/docs/reference/api). Newer T3 GitCafeCredentials source
   uses a CLI credential helper; that comparison is source-only.
8. **Other-host PR lookup remains unresolved:** desktop offers GitHub, GitLab,
   GitCafe, Forgejo, Bitbucket and Azure DevOps. Only GitHub/GitLab currently
   supply status PR metadata. The other-host real Connection/validator/actions
   regression records null provider/PR metadata. A changed pushable branch can
   still enable commit_push_pr because the reader tests Git state, not provider
   support; execution can then fail unsupported. Host API implementations and
   authenticated fixtures for these four hosts are still required. This is a
   confirmed consumed gap, not an unsupported claim that their APIs do not exist.

Release synthetic measurements (`rss-shell-audit-*.json`):

| Workload | Shell bytes | Archive latency | Server HWM |
|---|---:|---:|---:|
| 10,001 active + 1 archived + 10,001 newer deleted | 7,110,418 | 14.52 ms | 15,880 KiB (16.26 MB) |
| 375 total, 128 KiB message each, 1 archived | 279,744 | 1.10 ms | 14,824 KiB (15.18 MB) |

Both measured server workloads are below 50 MB. The large fixture has one archived
row, not 10,001 archived rows. The archive latency/HWM comparison is against the
previous 882.77 ms / 95,740 KiB measurement with the same large empty-thread shape.
The incremental reader test also replaces one latest message with 9 MiB before
updating metadata: the shell update remains under 8 KiB and readable.

Final validation: `cargo test --no-fail-fast` passes 226 tests with defaults and
224 without defaults, 17 ignored opt-ins each (including the new isolated pinned
probe). Strict Clippy `--all-targets -- -D warnings` passes both configurations;
`cargo fmt --check` and `tools/gen-compat` pass. Pinned 3991 general, concurrent
and P1-reader differentials pass; the P1-reader comparison reports equality.
The incremental fixture initially stalled because its direct SQLite writes did
not notify the WAL checkpointer; waking it fixes that test setup. No production
WAL policy was changed. All builds/tests run sequentially under nice with -j4.
No push, merge, service changes, real credentials, inference or emulators.

Final standard `tools/bench-rss` HWM: 16,544 KiB (16.94 MB), no inference;
see `rss-automatic-flow-audit.json`. Final binary hashes match all three measurements.

### Audit of the shell-size round (2026-11-02)

Fixed with a test that fails first: the base64 budget hit Codex images, which travel as a
path, so an accepted 7 MiB image failed its turn; FTS queries are now tried folded and raw,
because the index stores raw text and folding alone lost older fullwidth matches, while
decomposed accents need no index rebuild; a composite action that creates a pull request
on a recognized host other than GitHub now fails before commit or push instead of after
them, since creation goes through `gh`.

Known limits. A complete shell has no pagination: 10,001 threads with 80-character CJK
titles produced 9,109,889 bytes against the client's 8 MiB limit, so an owner-scale profile
far beyond today's 375 threads would need client changes. `getThreadProjection` and history
stay size-limited for a single multi-megabyte message, while the bounded mode both apps use
accepted a 9 MiB event. Status has PR metadata for GitHub and GitLab only. FTS ranking has
no wall-clock deadline. Credential-store-only GitCafe authentication stays unsupported.

A later check found the first version of that refusal read `upstream` while `gh` may use
`origin`, which would refuse a valid GitHub action. It now refuses only when every remote
is a recognized host without `gh` support (GitLab, Bitbucket, Codeberg, Azure DevOps,
GitCafe). Any unrecognized remote passes, so a self-hosted Forgejo can still fail at the
pull request step, after commit and push. Status still has no PR metadata for those hosts,
and the lib-t3-rust action menu has no capability to read; that part is client work.

## P1 promotion (2026-11-02)

`assets.createUrl`, `orchestration.searchThreads`, `orchestration.subscribeCommandOutput`,
`server.discoverSourceControl` and `thread.fork` are promoted on the last audit's verdicts,
of head ee795cf. That audit's only blocker, the remote read by the pull request refusal,
was fixed afterwards with a test; that fix itself was not independently audited. Partial
for a named reason: `vcs.refreshStatus`, `server.updateSettings`,
`orchestration.subscribeShell` and `server.getConfig`.

## P2 production reader inventory and CPU history — 2026-11-02

[Inventory](p2-reader-inventory.md) covers all partial non-P0/P1 rows, including
production desktop/Android call sites, user-visible effects and proposed actions.
Statuses and priorities are unchanged. This is an intentionally small implementation
block: usage/rates require ledger scanning/pricing work, devices require protocol
and lifecycle fixtures, and subagent stop still wrongly interrupts the owner.
Those remain pending, together with the other actions in the inventory.

`server.getResourceTelemetryHistory` had a confirmed consumed defect: bucket CPU
counted only the server, while bucket RSS included the whole child tree. Desktop
`process_diagnostics.rs:28` and Android maintenance render bucket CPU/RSS. The
real Connection and lib telemetry reader test `history_cpu_includes_busy_provider_children`
failed before the fix (bucket average 0.067%, peak 0.20%, child lifetime CPU 93.49%) and passes afterward.
The fixture is a busy shell, without providers, credentials or inference. Sampling
now sums per-process tick deltas keyed by PID/start-time, so exited children do
not subtract counters from surviving processes and reused PIDs get new baselines.
New children contribute only when born since the preceding sample. CPU from a
child that starts and exits entirely between samples cannot be recovered.

Collection remains on demand at 10 s intervals; no new startup work, timer, or
request frequency was added. Pre-open history is still unavailable. Per-process
IO and the desktop telemetry source were listed as gaps but are not read by the
named production bucket views. This block does not certify complete telemetry.

The new opt-in `upstream_p2_telemetry_reader` runs actual lib readers against both
isolated servers with a fixture terminal workload. Its first run exposed an idle
sampling setup error in the o3 probe; retaining a real report subscription fixes
that setup. Pinned 3991 returned zero retained samples, despite healthy native
status: its CPU comparison remains **unproven**, not equivalent. The probe records
that explicitly in [the report](p2-telemetry-report.json); if reference samples
are available it requires child CPU visibility too. It is separate from the two
existing pinned differentials, which retain their strict comparisons.

The storage notes were stale: worktree/browser artifact cleanup and automatic
invocation already exist (`tests/storage_policies.rs`, `scheduled.rs`, and the
ops disabled-policy test). Notes now describe that implementation without claiming
a new fix or automatic-stream parity. Editor callers currently open a workspace
or a keybinding file without line/column; missing goto equivalence alone does not
establish a client-visible failure. File-manager reveal ownership remains to audit.

Preview profile review: desktop gates the RPC on `serverBrowser`, which o3 does
not advertise, and clears local rendering profiles separately. No enabled o3
production RPC caller was found. The inventory recommends independent
reclassification of that row; its status and priority remain untouched.

Final validation: 231 default / 229 no-default tests pass, 18 opt-in tests ignored
in each configuration. Strict Clippy (`--all-targets -- -D warnings`) passes in
both configurations; fmt and gen-compat pass. Both pinned general/concurrent
differentials and the additional P1-reader differential pass. The P2 probe
records 3 o3 samples with a 33.27% bucket average and zero pinned reference samples;
it passes its o3 regression but cannot certify reference CPU equivalence.

`tools/bench-rss` release HWM is **16,408 KiB (16.80 MB)**, below 50 MB;
last-100 append median/p95 **2.503 / 3.316 ms**, metadata **0.433 / 0.522 ms**.
[Measurement](rss-p2-reader-block.json). This standard workload excludes inference,
client/build RSS and does not measure sampler overhead under many live children.
No startup or message-path work was added; the changed allocation/counter work
runs only in the existing on-demand sampler. No performance improvement is claimed.
Cargo builds/tests ran sequentially under nice with four jobs, in the required
/var/tmp build directories. No emulators, credentials, live inference, services,
pushes or merges were used.

## 2026-11-02 — P2 implementation continuation (incomplete)

This continuation does not close the requested 31-row inventory. No status or priority
was changed. The dated table in `p2-reader-inventory.md` records an outcome for each row,
including unfinished implementation work; those technical reasons are not external blockers.

Changes and real-reader negative controls:

- `tests/p2_stop.rs`: the pre-fix server accepted unsupported child cancellation
  (`Ok(sequence)` where the regression requires an error), reserving an owner interrupt.
  After removal of that fallback, actual lib dispatch/projection readers over a real
  Connection pass with deterministic Claude and Codex peers: child evidence remains running,
  the owner remains running and accepts a continuation to completion. The child record is
  seeded, so native discovery, task-specific cancellation and item cancellation are unproven.
  This is a safety fix, not independent cancellation support.
- `tests/p2_rates.rs`: real lib refresh/summary readers failed before implementation with
  pricing `unavailable` instead of `fresh`. A deterministic HTTP document now exercises
  bounded on-demand fetching, numeric-rate validation, atomic persistent cache, preserved
  fetch timestamp/count on failed refresh, and reload. Public rates are not applied to
  usage because ledger scanning/aggregation remains absent. Startup does not fetch rates.
  The former admin unavailable test initially fetched public rates and failed its stale
  assumption; it and the differential harness now use an unavailable loopback endpoint.
- `tests/pull_requests.rs`: actual lib list query/reader executes through the real Connection,
  checking combined reviewing/draft/review/checks/grouped-label/excluded-label/author/query
  arguments and client-consumed entries with scripted gh. This passed without a server fix;
  there is no failing-before claim or live GitHub equivalence claim.
- `tests/device.rs`: real lib action/detail reader failed before implementation with
  `this Android action is unsupported`. Fixture adb now covers appearance, font size,
  reduced-motion and network toggles, plus settings/foreground-package readback. Font-scale
  mappings were checked against newer source, not pinned behavior. No hardware, emulator,
  native settings behavior, encoder or video decoder was exercised. Detail uses optional
  adb queries; slow-device latency and unsupported settings remain unproven.

Validation: full default suite **234 passed, 18 ignored**; no-default-features suite
**232 passed, 18 ignored**, both with `--no-fail-fast`, `-j 4` and serial test execution.
Strict Clippy (`--all-targets -- -D warnings`) passes in both configurations; formatting,
`tools/gen-compat` and whitespace checks pass. Clippy found an incomplete-read pattern in
an HTTP test fixture; the fixture now reads bounded complete headers, with its real-reader
test rerun passing. Pinned 3991 general, concurrent, P1-reader and P2-telemetry differentials
pass in isolated profiles with deterministic peers. P1 reader equality is true; the pinned
telemetry probe again returned no samples, so CPU equivalence remains unproven. These
existing differentials do not certify new rate or device-settings parity.

Still unfinished: independent native subagent stop; local usage ledger discovery,
bounded parsing/deduplication/timezone/cost aggregation; rollback crash replay; webhook
schedules; staged clone/worktree setup and cancellation; ACP execution; remote/EAS/device
tool transports; shutdown and H264. External CLI updates of unknown ownership remain
subject to the owner's existing open decision. Unused/gated rows and existing cleanup
implementations are documented separately from demonstrated defects. No push, merge,
credentials, live inference, real installs, devices or emulators were used for the fixes.

Release `tools/bench-rss`: maximum VmHWM **17,492 KiB (17.91 MB)**, under 50 MB;
cold RSS 9,740 KiB, after-churn RSS 16,288 KiB. Synthetic 1,000-message/three-subscriber
workload, no inference and no provider-child RSS. Append p95 3.24 ms; metadata append
p95 0.53 ms. See `rss-p2-implementation.json`. This measures the ordinary startup/hot
path, not large ledger/rate-cache stress or native device latency; no performance
improvement or universal memory bound is claimed.

## P2 audit and promotion (2026-11-02)

The independent audit of head cbd63e8 reproduced two regressions, both past the clients' 12 s
read timeout: a rate refresh waiting on a silent source, and five sequential device queries.
It also found that `device.configure` wrote settings keys clients never read back. Each fix
has a test that failed first: refresh is bounded to 9 s and concurrent callers get cached
state; device detail shares an 8 s budget and returns partial settings; configure writes
`enableDeviceSupport` and `enableAgentDeviceAccess`. Device support is therefore off by
default, as in T3, until a client enables it. These three fixes were not audited on their own,
so their rows stay partial.

Nine rows are promoted and two move to unused on that audit's table.
[p2-reader-inventory.md](p2-reader-inventory.md) is the author's starting point; the row notes
carry the audited gaps and win where they differ. By user impact the audit ranks next: usage
aggregation, desktop device viewing and offered controls, child-scoped subagent cancellation,
staged setup with progress and cancellation, tracked cloning, webhook schedules, ACP execution,
archive size and rollback crash recovery.

## 2026-11-02 — Client-visible usage, hooks, rollback, archive and device block

This block is **incomplete** against the nine-item implementation request. No row status
or priority changes. Items 2–4 are still implementation work, not proven infeasible:
child-native cancellation needs task identity plus acknowledged native outcome handling;
worktree launch still prepares synchronously, without a staged script runner/cancellable
process ownership; clone still waits for git and has no durable job/subscription lifecycle.
Their safe refusals/capability gating are retained. This block did not finish those items
before moving on, so it does not satisfy the requested execution order.

Failing-before and passing-after evidence uses real Connection/lib readers:

- Usage: `tests/p2_usage.rs` first returned zero tokens from empty buckets; now the real
  production aggregate reads 280 tokens across Claude/Codex fixtures at the Santiago day
  boundary, recorded cost and an explicit unpriced count. Canonical provider turns avoid
  double-counting runs/attempts; imported turns count when they carry accounting/time.
  Historical run selection precedes session model fallback. New Claude records preserve
  cache-creation subsets; old records cannot recover that split. On-demand iteration has
  capped bucket/session aggregation and an explicit over-budget error. No provider ledger
  files are read. Missing turn accounting and native last-request-only counters cannot
  reconstruct full provider history; this is not complete invoice accounting.
- Webhooks: `tests/p2_webhook.rs` first rejected signed definitions. It now exercises actual
  lib upsert/list plus HTTP signature rejection, filters, replay dedupe, coalescing and
  expired queue suppression. Mandatory HMAC-SHA256 verification uses constant-time tag
  comparison; signing secrets are kept out of definitions/receipts. Bodies are limited
  to 1 MiB and reads to 5 seconds on the existing listener. Queues/replay keys are capped,
  persisted, and have no automatic retry. Direct delivery age starts at server receipt;
  unverified relay timestamps are not trusted. Credentials and pending jobs are removed
  with task deletion. No payload/secret logging is added. `secretRef`, relay verification,
  full template parity, delivery-log APIs and crash-safe queue handoff remain unfinished.
  The outbound HTTPS/private-target restriction does not apply to inbound triggers.
- Rollback: `tests/p2_rollback.rs` first retained stale metadata/partial files. A synced
  filesystem intent now precedes restore; SQLite metadata/receipt commit precedes intent
  removal. Startup replays intents before serving or resuming providers. Failed restores
  gate thread mutations until restart/recovery. The regression injects the crash window
  with durable intent, partial files and old database state, then reads recovered projection
  and receipt. It does **not** inject SIGKILL/power loss or cover a filesystem failure matrix.
- Archive: `tests/p2_archive.rs` first returned `rpc_outcome_unknown` for 1,000 archived
  threads with 9,000-byte titles. Incremental size accounting now rejects before 7 MiB
  with an actionable typed error. The actual lib archive request has no pagination fields;
  returning a partial successful archive would mislead the readers. Large complete archives
  still require a client/protocol change.
- Device: `tests/p2_device_stream.rs` first failed the real SEMU parser because reset-video
  returned an unavailable error. A socket-owned adb `exec-out screenrecord` now sends
  bounded H264 chunks in SEMU packets; disconnect kills/reaps its recorder and slow sends
  have a deadline. The fixture bitstream decodes to a 32×32 frame with ffmpeg. Actual lib
  open/action/close readers exercise rotation and shutdown; fixture argv covers native-size
  orientation mapping, emulator sensor rotation and adb shutdown. Mutation RPCs share a
  9-second deadline. No native device encoder, desktop UI, real device or emulator was run.
  Device behavior is fixture/newer-source evidence, not pinned device parity.
- ACP: the deterministic registry/npx fixture in `tests/provider_setup.rs` first returned
  `prepared:true`. The actual lib preparation reader now receives `execution_unavailable`:
  an available runner does not supply an ACP execution driver. Registry search remains.

Untouched requested exclusions: vcs.refreshStatus, server.updateSettings,
orchestration.subscribeShell, server.getConfig, and remote-host device rows.

Pinned validation: all eight isolated `upstream_` probes pass, including the added
`upstream_p2_webhook_reader`. Actual lib scheduled-task upsert reports a configured secret
without exposing it on both servers; direct HTTP invalid/valid/duplicate deliveries match
3991 exactly (401/rejected_signature, 202/accepted, 202/duplicate). This establishes those
observations only, not filter/coalescing/expiry/relay equivalence. The reference telemetry
probe again returned zero samples, so CPU equivalence remains unproven. No credentials,
inference, devices, emulators, services or user profiles were used.

Usage scan measurements (debug, real Connection): 50,000 accounting turns **848 ms**,
server scan **845 ms**, **681-byte** response. A 100,001-turn accounting-free history
returns an explicit scan-budget error in **739 ms**, rather than a partial zero summary.
The scan counts every turn, including those without accounting, and caps at 100,000 turns
or 8 seconds; larger profiles need indexed ledger/backfill work. Bucket/session allocations
are capped separately. This is a bounded refusal, not a claim that arbitrarily large history
is fully queryable. No transcript I/O, startup ledger migration, network rate refresh,
message-path ledger write or background ledger timer was added.

Final verification: **244 default / 242 no-default tests pass**, 19 opt-ins ignored in
each full run. Strict Clippy (`--all-targets -- -D warnings`) passes in both feature
configurations; fmt, gen-compat and whitespace checks pass. The eight pinned probes
pass, with the telemetry limitation above; [signed webhook observations](p2-webhook-report.json).

Release `tools/bench-rss`: **18,152 KiB** peak (**18.59 MB**), below 50 MB. Process-start to authenticated WebSocket: **12.12 ms** (10 ms readiness polling; profile pre-initialized by isolated auth CLI).
Last-100 append median/p95 **2.448/3.341 ms**, metadata median/p95 **0.447/0.570 ms**.
[Measurement](rss-client-visible-gaps.json). This is the standard synthetic workload,
without inference/device children or large usage/rate-cache stress. Startup adds an
intent-directory probe; thread mutations add pending-rollback checks. There is no
controlled startup baseline or performance-improvement claim. Large-cache/ledger
peak RSS and live video throughput remain unproven. Builds/tests ran sequentially
under nice with four cargo jobs, using the required build/temp directories.
No pushes or merges, including the local development journal.

## 2026-11-02 — Independent security audit follow-up

Statuses and priorities are unchanged. Local changes only; no push, merge,
credentials, live inference, device/emulator, or running-profile access.

- **Webhook authorization:** the contract signs the raw body only. Filters and
  prompt/coalescing templates accept only `body` fields; unsigned references fail
  definition validation and legacy delivery validation. Header values, including
  any capitalization of the signature header, never enter template data. A raw
  body digest permanently identifies a delivery for that hook. Replay records are
  retained until hook deletion; reaching storage budgets rejects new deliveries.
  There is no signed timestamp: sender age cannot be authenticated.
  `maxDeliveryAgeMinutes` bounds queue residence, not sender age. This limitation
  is intentional and must not be represented as authenticated freshness.
- **Who can trigger what:** anyone holding the hook URL/token and signing secret
  can submit signed body data to the configured task/thread prompt. Execution uses
  that task/thread's permission mode. Full access can mean Codex
  `approvalPolicy: never` with `danger-full-access`, or Claude
  `bypassPermissions`. Signing-key holders must therefore be trusted to trigger
  work with those permissions. Template framing is not a prompt-injection sandbox.
- **Rollback:** captured scopes and synced intents bind canonical workspace and
  git directory identities, thread, and project. Changed/symlink workspaces and
  legacy intents without bindings fail closed with a recovery error. A pinned
  directory fd prevents workspace path replacement from redirecting restoration.
  Files/directories are synced, the SQLite receipt uses `synchronous=FULL`, then
  journal removal is synced. Four seeded interruption states and one-shot
  failures after journal sync, file sync, receipt commit and unlink recover
  through real Connection/projection readers. Process-local flags are reset to
  simulate restart; real power loss and filesystem fsync errors are unproven.
- **Admission and dispatch:** read-only existence/authentication precede the writer
  and run on blocking workers. Limits are 30 requests/IP/s, 60 global/s plus
  existing connection caps. Unknown hooks reject before body reads. Dispatch
  freezes a durable queue claim and uses its stable receipt ID; failures retain
  the job, and receipt replay avoids launching it again. Simulated interrupted
  handoff is covered, including injected stops at claim and receipt with one
  projected run after replay; external provider side effects across real crashes are not
  proven exactly once. Logical queue/replay byte budgets complement count limits.
- **Usage:** cached-read savings use known input/cache prices; date/accounting
  predicates precede the bounded scan on a private read-only SQLite connection.
  Cached savings and 100001 undated-record narrow-query regressions both failed
  before and pass after through real client readers. The 50000-turn query took
  1.07 seconds in debug; loopback admission handled 40 invalid requests with the
  writer held in 11 ms, rate limiting 10.
- **Device:** an EOF fixture reproduced stream closure; restart preserves the
  viewer socket and delivers another SEMU packet. This uses fixture adb, the real
  protocol reader and ffmpeg, not real hardware. Desktop Shake remains unsupported.

Negative controls and final validation are summarized in
`docs/security-audit-validation.md`. Child-scoped stop, staged workspace setup
with cancellation, and tracked clone jobs remain outstanding; no feasibility
claim or status promotion is made.

Validation: default 251 passed / 19 ignored; no-default-features 249 passed /
19 ignored; strict Clippy both configurations, fmt and generated compatibility
notes pass. Eight pinned binary differentials pass, including observed acceptance
of unsigned query templates in 3991 and rejection here. Release HWM 18332 KiB
(18.77 MB), WebSocket availability 12.48 ms; synthetic profile, no inference.
See `docs/rss-security-audit.json` for the complete measurement and scope.

## Security recheck and merge (2026-11-02)

The recheck of b94f8ec confirmed the webhook and rollback fixes and found two blocking
issues that also exist on earlier main, so they are recorded on their rows instead of
holding this merge: rollback trusts a mutable checkpoint ref name, and a run whose receipt
committed before its provider started is failed on restart with an explicit reason rather
than executed. Exactly-once external effects are not claimed.

It also found three issues new to this branch, fixed afterwards with tests that failed
first and not audited on their own: unauthenticated traffic shared a rate bucket with the
real sender and could lock it out; unknown hooks and wrong tokens returned different
bodies, revealing which hooks exist; a recorder printing bytes before each failure could
restart forever. The token is now checked before any body is read, every rejection is the
same `404 {}`, only token holders count against a 60 per second budget, and the recorder
restarts at most three times per minute.

## Setup, clone and child-stop attempt left unmerged (2026-11-02)

Three audit rounds tried staged worktree setup, tracked clone jobs and child-scoped
`subagent.stop`. The final recheck of fa5ab7f reproduced two serious failures: a setup daemon
that clears its environment survives cancellation and restart recovery, and one clone's cleanup
can delete another clone's successful, registered checkout. Corrupt setup state also blocked
startup. The branch `t3/implement-client-visible-flows` is kept unmerged. Main keeps the safe
behaviour on these rows: synchronous setup, cloning disabled, and a stop rejection that leaves
the owner turn running.

## 2026-11-02 — External updates and system-browser implementation (incomplete)

No status or priority changes. See [reader inventory and outstanding contract](provider-browser-contract.md).
This work does not complete the owner's requested agent-browser scope.

External updates: removed unsafe blanket `claude update` advertisement/execution.
A standard user-owned npm global prefix with matching native package metadata
can be updated explicitly through the existing server.updateProvider RPC, using
its original npm and prefix. Unknown, Homebrew, system-owned and other-driver
installations remain manual advice. No sudo, shell execution or automatic update.
The child has bounded time/output, process-group cancellation and a bounded
post-update version probe. Raw installer output is suppressed, including errors.
The existing config and install-stream readers receive state and operation IDs.
Automatic Homebrew/native/other-driver ownership resolution is unfinished.

Browser: lazy system Chromium child, inherited CDP pipe, private hashed profile
storage, isolated incognito storage, attached page, basic navigation/resize and
JPEG viewer with bearer/media-ticket authentication, ACK pacing, send deadline,
session revocation checks and bounded lifetime/output. Profile clear stops and
reaps the process before removing storage, rejects symlink roots, and closes
snapshots. Close and server shutdown stop children. No browser is downloaded;
missing executable returns an actionable message. One page per persistent profile
is currently enforced. `agentBrowserAccess` stays false: automation tool/broker
integration and the remaining browser protocol are unfinished.

Failing-first evidence: the real lib provider projection initially returned
canUpdate=true for an unowned Claude fixture; it now returns false and refuses
the RPC. The real clearProfile reader initially acknowledged a symlinked profile
root; it now rejects it and preserves the outside sentinel. Additional real
Connection tests cover fixture npm argv/prefix, installer failure, active cancel,
output suppression, CDP session routing, authenticated JPEG bytes, anonymous
viewer rejection and reaped child before clear. Fixture child RSS observed
11,160 KiB; this is Python, **not Chromium RSS**. System Chromium rendering,
Chromium child RSS, real package updates and native agent tool access are unproven.
No network, inference, credentials, user profiles, services, emulators, push or
merge were needed. Newer T3 source observations are labelled in the inventory;
existing pinned probes cannot certify the new browser/updater engine behavior.

Validation: 255 default / 253 no-default tests pass, 19 opt-ins ignored in each
full run. Strict Clippy with all targets and warnings denied passes in both
configurations; fmt and gen-compat pass. All eight isolated pinned differentials
pass (177.85 seconds). The telemetry reference still provides no CPU samples;
these probes make no new browser or external-update parity claim.

Release `tools/bench-rss`: maximum sampled server HWM 17,900 KiB (18.33 MB),
under 50 MB, with browser unused and no browser child. Process-start to
WebSocket 11.99 ms. Last-100 append median/p95 4.547/5.592 ms; metadata
median/p95 1.970/2.235 ms. These are synthetic workload observations, not a
controlled performance comparison. [Measurement](rss-provider-browser.json)
separately records the Python fixture child and explicitly leaves actual
Chromium RSS unmeasured. No combined-server/browser memory claim is made.

## 2026-11-02 — Verified installers and scoped browser broker follow-up

Statuses and priorities remain unchanged. See
[the implementation record and outstanding gaps](provider-browser-follow-up.md).
The broader browser contract remains incomplete; the new capability means that
scoped core browser tools can be delivered to providers, not that every upstream
preview tool is implemented.

Maintenance adds verified user-owned Homebrew core formula receipts, Claude's
native versioned symlink installer and OpenCode's native binary layout. npm
updates now recognize additional configured OpenCode (v1/v2), Pi and Grok package
identities, including unscoped global packages. Unknown ownership, unrecognized
casks/custom taps and other standalone layouts stay manual with original-installer
advice. There is no sudo, shell command construction or implicit update. Existing
execution bounds, cancellation, output suppression and version verification apply
to each plan. A running CLI version is verified; registry-latest is not certified.

The browser broker issues a per-run bearer-protected loopback MCP listener and
injects it into native Claude/Codex configurations. Global and project settings
are checked when issuing access and on calls. It exposes status/open/navigation,
close/resize/evaluate, textual snapshot and basic CSS/key interactions. Agent
profiles are private ephemeral directories; scope guards close and reap owned
tabs at the end of a run. Persistent-profile tabs share a browser process with
separate attached CDP sessions. Viewer input has an exclusive human controller;
agent mutations reject a currently human-controlled tab. History reconciliation,
back/forward viewer input, appearance, zoom and cache/cookie/reload operations
now reach CDP. HTTP(S) development URLs, including local network addresses,
follow newer source behaviour; explicit file navigation is rejected and CDP
is configured with file/FTP block patterns. Actual Chromium enforcement is unproven.

Failing-first evidence: the real Connection/config/projection test rejected the
Homebrew fixture as non-updatable before implementation; the same test now passes
for Homebrew and native Claude, checks no update on config reads, then rejects a
removed receipt. The real config reader initially returned agentBrowserAccess=false
with an available fixture; it now returns true and respects disabling settings.
Native peers on both transports initialize and call the real broker, reject a
wrong bearer/file URL/foreign tab, then receive no browser access on the next
turn after a project override. The lifecycle regression checks two tabs sharing
one persistent context and a surviving process after closing only one tab.
These are fixture tests, not real provider inference or real installer/browser
acceptance. No real CLI update, credentials, user browser profile, service,
emulator, external network access, push or merge was used.

Validation: 258 default and 256 no-default-features tests pass, with 19 ignored
in each configuration. Strict Clippy (`--all-targets`, warnings denied) passes in
both configurations; fmt and regenerated compatibility notes pass. All eight
isolated 3991 differentials pass (176.32 seconds). No new pinned-browser or
external-updater equivalence claim follows from those probes.

Release `tools/bench-rss` with an explicitly enabled Python CDP fixture and an
isolated PATH (no real provider CLIs) measured server RSS/HWM **17,920 KiB** with
the browser fixture open and idle. Across the synthetic 1000-message, 20-thread,
three-subscriber workload, maximum sampled server HWM was **18,952 KiB (19.41 MB)**,
below 50 MB. The separate fixture child RSS was **11,164 KiB**, not Chromium.
Startup to authenticated WebSocket was 12.04 ms. Append last-100 median/p95 was
4.401/5.123 ms; metadata median/p95 was 1.898/2.172 ms. These are observations,
not a controlled performance comparison. No real Chromium was run; actual
Chromium rendering, sandbox/dependencies, file/redirect enforcement and child RSS
remain unproven. [Measurement](rss-provider-browser-follow-up.json).

Remaining work is explicit in the follow-up: semantic/ARIA/frame and advanced
browser tools, selectable saved-login contexts/reveal details, full unsolicited
load/dialog/download events, takeover races and crash recovery. These are not
proven infeasible. Unrecognized maintenance ownership stays manual, not guessed.
This implementation does not justify declaring the entire owner's scope complete.

## 2026-11-02 — Independent audit remediation

This follow-up addresses the audit of 31fe7f1. Compatibility row statuses are
unchanged; the independent auditor decides whether to promote them. Row notes
describe the narrower evidence below.

- Claude's browser bearer was present in the fixture child's proc cmdline. The
  same peer now reads the private MCP JSON file, verifies mode 0600 and parent
  mode 0700, and asserts the bearer is absent from proc cmdline. The file guard
  belongs to the session and removes its directory on teardown or spawn failure.
  Installed Claude CLI help confirms that --mcp-config accepts JSON file paths.
  Codex receives MCP configuration in its app-server thread/start protocol,
  covered by the existing native transport peer, rather than command arguments.
  An actual native MCP handshake is still unproven.
- A standard-layout package with npm pointing at the privileged su executable
  was accepted before the fix. The regression now rejects it before execution.
  Installer selection canonicalizes paths, checks regular executable files,
  owners, parent-directory permissions and setuid/setgid bits, confines symlink
  destinations, and performs bounded tool-specific version checks. The same
  checks apply to brew and native installers. External install/version execution
  uses a reduced environment and a fixed interpreter PATH. UID-1000 fixture
  npm/brew/native updates pass; no real CLI update was performed.
- Initial freeform/preset viewport requests previously produced no metrics
  command. CDP now acknowledges initial metrics before open returns, and a failed
  metrics command closes the reserved tab. Fill leaves browser-managed sizing.
- Eight exited fixture browsers previously remained in list and blocked open.
  A completion watcher now removes snapshots and publishes closed revisions;
  completed actors are excluded from capacity accounting. The real lib list/open
  readers observe cleanup and a successful replacement open.
- A fixture Page.navigate errorText previously yielded success. It now reaches
  the actual lib navigate reader as an RPC error without publishing a successful
  navigation snapshot.
- Default tests previously included the pilot integration that starts a user
  unit. It is now ignored and additionally requires O3_TEST_PILOT=1, including
  when ignored tests are selected. This integration was never invoked during
  remediation. Differential children now arm Linux parent-death SIGKILL with a
  parent-race check. A regression that kills the harness reproduced the orphan
  before the change and observes child termination afterward. Startup removes
  recognized stale profile directories only when their harness PID is absent.

Subprocess review also covered providers and auth flows, config-source curl,
updaters and version probes, browser launch, git/gh, file opening and PTYs.
Config-source authorization and request bodies already use curl stdin; provider
environment credentials stay in the environment, auth responses use stdin,
browser URLs use CDP, and PTYs use a separate environment. No other o3-created
bearer argument was found. User-supplied command/launch arguments, URLs and git
configuration remain user-controlled inputs, not a general secret-redaction
boundary. File permissions do not protect against the same UID or root. Installer
layout/version checks do not authenticate arbitrary same-user executable
replacement or close filesystem races. Parent-death signaling covers the direct
harness child, not descendants deliberately detaching themselves.

Final verification: default tests 265 passed, 0 failed, 21 ignored; without
default features 263 passed, 0 failed, 21 ignored. Both full runs used one test
thread and loopback-only network namespaces. Installer success paths additionally
passed three UID-1000 fixture integration tests, and the inherited-stdout probe
regression passed as UID 1000 (3 seconds before, approximately 10 ms afterward).
Strict clippy with all targets and warnings denied passed in both configurations;
format checking and tools/gen-compat passed. All eight pinned nightly-3991
differentials passed, including concurrent and P1 readers. These comparisons
retain their documented coverage limits: reference CPU samples were absent,
so CPU equivalence is not established. No ignored live/pilot test was invoked.


The opt-in system Chromium smoke also passed inside a loopback-only namespace
with an isolated ephemeral profile and a static loopback page. The authenticated
JPEG immediately after open measured 640 by 480, matching open metadata; resize
also measured 640 by 480, list observed the page title, and close/shutdown
completed. This is real-engine rendering evidence, not fixture rendering.
Real-engine navigation-failure parity and long-duration expiry remain unproven.

The release tools/bench-rss workload passed its decimal 50 MB server limit:
cold RSS 11,444 KiB, final RSS 17,752 KiB, HWM 18,292 KiB (18.73 MB), and
startup to authenticated WebSocket 12.63 ms. It seeded 1,000 messages, 20 threads
and three subscribers without inference. Browser-unused RSS excludes Chromium.
The separate browser-open sample measured server RSS 16,880 KiB and summed
Chromium descendant RSS 1,096,020 KiB across 13 processes; that aggregate double
counts shared pages and is not PSS. These are single samples, not a performance
regression study. The measured release source was 7907bb1; binary SHA-256:
fcb41db40d88bb856d61712b4a1aa5d9845a2ecd25c016cb1f709323c75b008e.

## Provider updates and agent browser merged (2026-11-02)

The owner put `server.getConfig` fully in scope. After two audits and a final recheck of
63ab15d, it is promoted: the bearer no longer appears in any child's argv and privileged or
foreign-owned updater layouts are refused. Native CLI MCP acceptance and real updates were
not run and are recorded as evidence limits. Three preview rows go back to partial for
defects the recheck demonstrated: initial layout at the wrong width, pinch zoom instead of
T3's layout zoom, and refresh without a real-reader test. Four crashed MCP tabs still block
new tabs at the limit of four. These are defects of a feature main did not have, and agent
browser access defaults to on as in T3.

## Claude request withdrawal parity — 2026-11-03

Upstream41588d452 closes approval and question artifacts when the SDK aborts its
permission callback. The direct transport maps control_cancel_request only for a
known pending request, removes it from the response map, and persists cancelled
request/node/card artifacts before a following result. Duplicate/unknown withdrawals
are inert; an already admitted late client answer cannot kill the provider session.
A dedicated request node replaces the prior reuse of the run root. The run stays
waiting until its last pending request is gone. No approval is inferred.

`tests/claude_withdrawal.rs` drives a deterministic Python stream-json peer through
the real Connection, permission/question readers, SQLite projection and event log.
Two concurrent requests cover duplicate/unknown cancellation, late rejected answers,
and a withdrawal immediately followed by completion. The new regression fails on
the previous adapter; it passes after the port. This is native-peer fixture evidence,
not an observed CLI hook event or live T3 differential. Existing approval/response
and session-release regressions also pass; full nightly interchangeability is open.

## Nightly 4106 review and upstream references — 2026-11-03

Port071660e0b: the temporary review index excludes staged deletions from files to
re-add as intent-to-add. The deletion scan uses --no-renames, so a same-content
staged copy cannot hide the old path. The real index and working file remain
byte-identical. The real-Connection regression covers plain deletion and rename,
fails on the previous implementation and passes after the change. Unborn
repositories use git diff --cached without explicit HEAD, keeping their existing
review behavior. All existing VCS/review fixtures pass.

Case064cf8f7a is already handled through Git porcelain v2 branch.ab counters; no
ambiguous short upstream reference is re-resolved for divergence. A real repository
with both a local origin/release branch and a release tag verifies refName and
+1/-0 relative to the real remote. This is covered behavior, not a new code port.

Checkpoint optimization07a1e799d remains open: o3 uses a fresh private index, so
real-index flags do not suppress edits, but rehashes the whole workspace. Project
content search8430db673 and forceStopThread remain open. No M2 migration, live
provider hook or latest-nightly differential is claimed by these fixtures.

Request-node expiry follow-up: release of an unanswered request now cancels its
owned approval node as well as its card. The session-release regression asserts
this and fails before the fix. Run roots are never cancelled by that cleanup.

Nightly monitoring was refreshed to4119/cefb544af3c98224874419198c0359b652ca3c94.
New sourceControl.star and the4119 session/import/subagent changes remain pending
triage/implementation; the monitor report is not a compatibility certificate.

CI fixture isolation: background/common now use the same per-client-thread
SIGCHLD shield as support. In-process Tokio subprocess handling can otherwise
interrupt the blocking native client's socket read, reported as rpc_outcome_unknown.
A Linux regression targets that thread with a non-restarting SIGCHLD handler:
unshielded read returns Interrupted, shielded read receives the byte. The masks
and handler are restored in the dedicated fixture process. No RPC is replayed
and no production signal policy is changed. Remote CI still must confirm the
background/VCS failures are resolved on the runner.

### Resumed Claude notification boundary (2026-11-03)

A real migrated session exposed a completed run with no assistant output while the native
CLI continued generating the answer. Its result usage contained zero new tokens. The
original result's origin was not retained, so this incident alone does not prove its exact
wire subtype. The adapter accepted every success result as the submitted prompt's terminal.
Upstream ClaudeAdapterV2 explicitly excludes zero-turn task-notification results.

The decoder now drops successful `origin.kind=task-notification`, `num_turns=0` results,
including their unrelated usage; human command results, real notification turns and errors
remain terminal. Result usage records retain origin, turn count and subtype for diagnosis.
A red/green replay and the fake native peer in `tests/fixtures/claude/notification-result.py`
exercise an unrelated result followed by the actual reply through the real provider and
projection. This is a bounded fix, not full prompt-UUID correlation or proof that the
already-affected live session is repaired.

Migration still imports messages/items without the original runs/attempts/checkpoints.
Native bindings alone therefore do not preserve execution history or run ordinals. Existing
long-lived threads and `/mcp` orchestration remain outside validated migration acceptance;
a short new-thread smoke test is insufficient evidence for that use case.

### Large handoffs and cold Claude compaction

Historical upload descriptors are supplied as file references alongside handoff text, so old
images remain readable without re-encoding all of them into the next inference request.
Current user attachments retain their normal image/file handling and limits. Codex outgoing
RPC frames allow 16 MiB to carry the existing 8 MiB input budget plus its envelope; incoming
frames retain their independent 1 MiB limit.

For a Claude `/compact` request after a provider change with a pending historical handoff,
o3 first requests a continuation summary in a persistent session with tools and MCP disabled,
then sends the literal native `/compact` command. Completion is emitted only after that
second result. The limited process is closed afterward so subsequent ordinary turns resume
with their usual tools. A deterministic native fixture verifies both phases and tool policy.
This does not implement Codex native compaction or change the general migration importer.

### Final Codex input character budget

Codex also enforces an aggregate 1,048,576-character text-input limit independent of
JSON transport size. A near-1-MiB handoff plus historical file references and the current
request can exceed it even after Claude compaction, because a provider switch reconstructs
context from the retained application transcript.

Composition now accounts for the final text blocks, preserving the entire current request,
context and attachment descriptions. Overflowing historical context is retained atomically
in a private content-addressed file; the inline message includes its path, an explicit
abbreviation notice and a UTF-8-safe recent excerpt within the remaining budget. The original
transcript is not changed. Current user input which alone exceeds the native limit is
rejected explicitly rather than silently shortened. Tests enforce the native text limit in
the Codex peer, exercise a completed continuation turn with an oversized handoff, and verify
that Unicode history remains byte-for-byte readable from the retained file.

### Nightly workspace content search (2026-11-04)

`projects.searchContents` is now routed with its existing `filesystem:read` scope.
The implementation follows the contract and fixtures in nightly 4119 source
`cefb544af3c98224874419198c0359b652ca3c94`, notably `packages/contracts/src/project.ts`
and `apps/server/src/workspace/WorkspaceEntries.test.ts`. It searches file contents
with literal/regex, case-sensitive and whole-word options; whitespace in the query
is significant. Match offsets are UTF-16 code units, including after astral letters.
Invalid or unsupported regex falls back to a literal query and returns
`regexFallbackError`. Zero-width matches are omitted.

At most two blocking workers scan fresh files using the Rust ignore/regex libraries. It honors
ignore files, includes hidden files, excludes `.git`, does not follow symlinks and
skips binary/non-UTF-8 content. Search results are bounded to 500 lines overall and
100 lines per file. Additional o3 budgets are 250 ms of scan time, 1 MiB per file,
64 KiB per line and 2 MiB serialized items; skipped oversized content or an exhausted
budget sets `truncated`. The deadline is cooperative between filesystem operations
and lines, not a hard deadline for slow filesystem calls. This is not upstream's
persistent fff index, ranking, or a guarantee of full regex dialect equivalence.

The real Connection regressions failed before implementation with RpcNotImplemented.
They cover Unicode and punctuation word boundaries, case/regex escapes, whitespace,
late whole-word matches, ignored/binary/external symlink content, per-file/global
limits, fresh edits, fallback and oversized content. Unit tests check typed errors
and schema bounds. No live nightly differential or desktop/Android UI acceptance
has been run for this port; the compatibility row remains partial.

### MCP transport and readers (2026-11-04)

See [MCP.md](MCP.md) for the separate MCP credential audience, transport contract,
published reader tools, protocol regressions and explicit gaps. `/mcp` now exists;
it does not yet implement all upstream toolkits or fix existing external integrations
automatically. The older browser-only per-run broker is independent. Full nightly
MCP parity remains unproven.

### MCP thread control and transaction-time admission (2026-11-04)

Thread tools reuse the ordinary dispatchers. MCP authority is not accepted in wire
commands; current credential/target modes are checked under the SQLite write lock,
before replay or mutation. Receipts and native delivery preserve the ordinary outbox
boundary, with agent/mcp provenance for submitted messages. New tests cover send
deduplication, queue edits/order/cancel, steering/promotion, question-only responses,
interrupt/wait and mode refusals. The stale-authority unit test covers both dispatch
paths. See MCP.md for exact tools and remaining restart/launch/provider-registry gaps.
This is still partial MCP parity; no daily profile or service deployment occurred.


### Provider-owned MCP injection (2026-11-04)

Normal supervised Claude/Codex turns now receive the shared MCP endpoint, with
process/session-owned in-memory credentials and current calling-thread authority.
Browser disable retains orchestration; foreground completion cleans scoped browser
tabs without expiring a cached CLI's credential. Explicit shutdown and observed
process exit revoke credentials and transport sessions, even with request references
remaining. Claude configuration stays in a private temporary file; Codex configuration
travels over app-server stdin. Native tokens cannot authorize ordinary HTTP/WS routes.

Three registry/policy regressions and real subprocess peers exercise credential reuse,
current modes, background-provider ownership, default targets, browser disable, abrupt
exit/replacement and shutdown. This supersedes the earlier private browser listener's
per-turn lifecycle and the provider-registry gap above. Source reference: nightly4151
322f6544dfad0a3c8a0c8cbecf2c7f7874b14268, McpSessionRegistry/McpInvocationContext/threadAccess.
See [MCP.md](MCP.md) for exact behavior and limits. The full preview/project/task/PR/device/
HTML toolkits, OAuth/DPoP/SSE, linked environments, remaining providers and live client
acceptance remain incomplete; this does not establish complete nightly parity.

Validation for this block: 319 default / 316 no-default pass; 21 opt-ins ignored each.
Strict Clippy in both modes, fmt, generated compatibility and nine nightly-monitor
tests pass. No service deployment occurred.


### MCP projects and workspace inspection (2026-11-04)

Four project mutations and two worktree readers now use the shared MCP transport.
Project writes require full-access/default and recheck authority under the SQLite
transaction, before replay. The ordinary project path also enforces atomic workspace
uniqueness, nonempty deletion, pin/archive fields, monogram graphemes and script-ID rules.
Cloning is shared with sourceControl.cloneRepository, stays separate from registration,
and supports raw remotes plus GitHub gh lookup/protocol fallback. Native and external
worktree readers share existing ref pagination and the missing-worktree fallback.

The existing readers/thread-control tests remain; new MCP/Git tests exercise actual
settings, files, checkouts, native defaults and rejected authority. A subprocess host
fixture verifies protocol selection and failure retention; a transaction regression
covers stale authority and receipt replay. See [MCP.md](MCP.md) for behavior and limits.
Worktree handoff and thread launch remain pending with provider detach/setup/continuation;
other-host repository lookup and full project enrichment still need source parity.
No service deployment or full-nightly conformance is asserted.

Validation: full suites pass with 326 default / 323 no-default tests (21 opt-ins
ignored each). Final MCP/project/clone/native regressions and strict Clippy pass in
both configurations; fmt, generated compatibility and nine nightly-monitor tests pass.
The existing compatible desktop and live hosted-network flows were not exercised.

Source audit also found a required integration follow-up: force deletion currently
marks child records deleted, whereas ProjectService.deleteChildThread plans durable
thread-deletion effects. Native/session/run cancellation and resource cleanup must
be aligned before integrating the draft project port. This gap is preserved explicitly
rather than treating CRUD persistence tests as proof of the whole deletion lifecycle.


### Durable deletion integration follow-up resolved (2026-11-04)

This supersedes the force-deletion gap recorded above. Ordinary thread.delete and
forced project deletion now share receipt-atomic cancellation and a durable resource
outbox, following nightly4151 ThreadDeletion/ResourceCleanupService/EffectWorker.
Native MCP is revoked, active work stops, cached Claude sessions are reaped and affected
Codex native threads are unsubscribed without stopping shared-process siblings. The
outbox retries failures/restarts and legacy tombstones are planned before run recovery.
Terminal history, previews, scheduled tasks and attachment sidecars are cleaned;
attachments shared by live threads and checkout folders are retained.

Four new integration tests exercise actual HTTP MCP and native-protocol subprocesses,
including active self-deletion, retired pools, failed unsubscribe, late binding replies,
atomic receipt rollback/replay, startup recovery and partial filesystem failures.
Full suites: 330 default / 327 no-default pass, 21 opt-ins ignored in each. Final
Codex/deletion/provider/MCP/background/browser regressions and strict Clippy pass in both
modes; fmt, generated compatibility and nine monitor tests pass. Source rechecked at
06:06 UTC remains nightly4151 (322f6544dfad0a3c8a0c8cbecf2c7f7874b14268).
Installed-provider inference, compatible desktop acceptance and the broader nightly
protocol remain unverified/incomplete; no service deployment occurred. See [MCP.md](MCP.md).


### Workspace metadata transitions (2026-11-04)

Ordinary thread.metadata.update now enforces expectedWorktreePath, expectedBranch and
expectedEmpty under the receipt transaction and plans a durable native detach on a real
path change. Live work is interrupted; queued messages wait behind the detach barrier
and resume the same native conversation in the new cwd. Shared Codex credentials and
sibling threads survive; exclusive Claude sessions restart with --resume. No-op/title
changes leave sessions attached. A separate durable queue-release marker covers a
restart after native detach; failures retry and late native bindings are unloaded.
Source: nightly4151 Orchestrator, ProviderSessionManager and ProviderSessionTransitionPolicy,
confirmed at 06:15 UTC (322f6544dfad0a3c8a0c8cbecf2c7f7874b14268).

Tests inspect actual RPC/MCP and subprocess native-protocol traffic plus transaction
rollback, expected-state races and persistent restart boundaries. This completes a
prerequisite for MCP handoff, whose branch/base/create/setup/continuation service remains
pending; no new MCP tool or full-nightly conformance is asserted. See [MCP.md](MCP.md).

Validation for this block: full suites pass with 333 default / 330 no-default tests,
21 opt-ins ignored each. Final transaction/recovery module tests plus 32 workspace,
deletion, queue/thread-command and HTTP MCP regressions pass per configuration.
Strict Clippy passes in both modes; fmt, generated compatibility and all nine
nightly-monitor tests pass. No service deployment occurred.


### MCP worktree handoff (2026-11-04)

The handoff-pending audit entries above are superseded: t3_worktree_handoff now creates
and binds a managed checkout, preserves remote/local base metadata, launches the effective
setup terminal by default and durably stages its optional continuation with the binding.
The shared workspace-transition lifecycle handles native detach and queue release.
Authority is rechecked after checkout; concurrent calls are serialized per thread and
binding failures roll back only owned Git resources. Transport disconnect does not lose
the admitted operation. Setup/continuation failures remain explicit fields on a successful
binding result; setup started does not imply command exit success.

Seven integration tests inspect actual HTTP MCP, real Git repositories/remotes and
subprocess native-protocol peers, including both Claude/Codex conversation identity,
setup environment/files/terminal, remote aliases/stale refs, atomic receipt rollback,
partial follow-up failures, UI races, authority loss and disconnected callers.
Source: nightly4151, 322f6544dfad0a3c8a0c8cbecf2c7f7874b14268, checked 06:36 UTC;
WorktreeMcpService, GitVcsDriverCore/GitManager, ProjectSetupScriptRunner/projectScripts
and worktree contracts. See [MCP.md](MCP.md). Full-nightly differential and compatible
client acceptance are still pending; no service deployment occurred.

Validation: full suites pass with 340 default / 337 no-default tests, with 21 opt-ins
ignored each. Final workspace/deletion/MCP-project regressions and strict Clippy pass
in both configurations; fmt, generated compatibility and nine monitor tests pass.
The opt-in live_worktree_handoff example also passed with installed Claude Haiku and
Codex gpt-6.1-sol: each called the actual MCP handoff, ran setup and wrote a code remembered
from before the move in its queued continuation. Event history contains one distinct
native conversation ID per proof; both have interrupted then completed runs. This is
an isolated server/native-CLI acceptance proof, not full compatible-desktop acceptance.


### Same-run Codex interrupt/restart (2026-11-04)

The earlier restart_active/MCP restart gap is closed for same-instance Codex turns.
Source is nightly4151, `322f6544dfad0a3c8a0c8cbecf2c7f7874b14268`:
Orchestrator.dispatchSteerIntoRun, ThreadManagementService.latestSteerableRun and the
provider adapters. The source advertises interrupt/restart for Codex and refuses Claude.
The durable operation retains the run ID, supersedes the old attempt and creates an
ordinally increasing steering_restart attempt/root. It waits for the old native terminal
and executor drainage before submitting the replacement in the same native conversation.

Four HTTP/native-peer integration tests and two module tests cover same-run identity,
MCP caller scope with a newer queued run, same-instance model changes, receipt atomicity
and replay, preserved app-owned delegation, attempt-separated output/checkpoints,
stale frames/requests, capability/maintenance rejection, failed/missing acknowledgement,
Stop/delete/archive races and startup recovery. Before native admission, startup recovers
the exact durable replacement; after ambiguous admission it fails explicitly and preserves
the message without replay. Old executor fallback, output and checkpoint writes are
fenced by attempt ownership. See [MCP.md](MCP.md) for delivery bounds.

The opt-in live_run_restart example passed with installed Codex gpt-6.1-sol: the replacement
recovered a code from earlier native context, with one run, two attempts and one native
conversation ID. This is isolated native-CLI evidence. Full-nightly differential and
compatible desktop/Android acceptance remain pending. Cross-instance active handoff,
thread launch/transfer and cross-thread restart-continuation following remain open;
this does not establish full protocol parity.

Validation for this restart block: full suites pass with 346 default / 343 no-default
tests, 21 opt-ins ignored in each configuration. Strict Clippy passes in both modes;
fmt, generated compatibility and all nine nightly-monitor tests pass.


### Linux setup descendants and launch prerequisite (2026-11-04)

The failed setup-cancellation attempt recorded above is superseded for Linux process
ownership: worktree-handoff setup terminals now use an isolated subreaper, with an
owner pipe rather than environment tokens or shared process-group assumptions. It
reaps descendants that double-fork, setsid and clear their environment. Its separate
session survives a group signal to the server; owner EOF triggers cleanup. It blocks
on owner/child events rather than polling. Setup terminal close and thread cleanup
await reaping; ordinary terminal process semantics and other targets are unchanged.

Two module tests cover admission failures and daemon cleanup on cancel, owner drop,
normal shell exit, SIGKILL owner and SIGKILL owner group. The subprocess owner test
kills an actual Rust owning process. An HTTP integration uses real Connection, native
Codex protocol peers and two worktree setups: closing one returns only after its daemon
is absent from /proc, while the other's daemon remains alive, then is closed separately.

This was the prerequisite checkpoint for ThreadLaunchService. Staged preparation,
tracking/cancel, completion and retry/recovery were still pending at PR24; the next
checkpoint below implements that shared RPC service. MCP launch remains pending.
The source intake is nightly4165, SHA
8b44f42b14c8e273156474a924ccf29680dd7857 (08:04 UTC). The launch/setup service and
tracker/contract are unchanged from4151; native runtime recovery and awareness projection
changes require follow-up. See [thread-launch-intake.json](nightly/thread-launch-intake.json).

Validation: full suites pass with 349 default / 346 no-default tests, 21 opt-ins
ignored each. After the final signal/event handling change, all library and handoff
tests pass again in both configurations (102 passed, 5 opt-ins ignored per mode).
Strict Clippy passes in both modes; fmt, generated compatibility and nine monitor
tests pass. No service deployment occurred.


### Shared staged launch and setup lifecycle (2026-11-04)

This supersedes the earlier synchronous workspace preparation and atomic-all launch
claims in this document. ThreadLaunchService's durable thread claim commits first;
initial message/context/run/events and its preparation job then commit atomically.
A rejected message keeps an empty claim, and retry admits that message once without
prior workspace effects. Accepted initial runs read `preparing` before Git/setup;
release records provider execution intent only after the selected setup policy.

The shared service prepares remote/local bases, managed checkouts/submodules,
binds workspace metadata, observes setup completion, and releases the initial run.
Synchronous failure retains its terminal/checkout for inspection and same-run retry.
Async setup releases the agent while tracking continues. Shell readiness uses live
bracketed-paste events or the quiet fallback, and keeps waiting on startup child
prompts. Setup uses the Linux scope prerequisite above to reap escaped descendants.

Worktree snapshots now contain ordered stages, bounded UTF-16 detail/tail/error,
monotonic sequence and finished retention. Cancel is effective before release and
returns false afterward. Checkout/ref rollback is restricted to owned resources;
shared/external workspaces remain. Failed cleanup is durable and gates retry.
Temporary branch generation follows naming/project override/collision rules, with
rename intent persisted before Git mutation. Managed Scratch launches allocate a
non-Git folder once. Preparation recovery resumes only unstarted work and explicitly
fails uncertain effects without replaying a potentially started setup script.

`tests/staged_launch.rs` provides ten HTTP/real-builder/native-peer regressions:
both providers' sync setup ordering and replay, escaped daemon cancellation plus
held queued input/retry, async release, sync failure inspection/retry, shell startup
prompt, Scratch replay, branch policy/collision, physical rename followed by rejected
metadata, locked checkout cleanup and project settings inside worktrees. Two module
regressions reopen SQLite for safe resume/explicit retry and corrupt request/snapshot
JSON, including valid JSON with an invalid type. Existing launch-reader, storage,
scheduler and native-import regressions now follow deferred admission and source
claim/replay semantics. Storage collection and async checkout binding share a gate.

`examples/live_staged_launch.rs` passed using installed Claude Haiku and Codex
gpt-6.1-sol. Each accepted one preparing run, waited for a gated synchronous script
without provider turns, then used the native provider to copy the prepared checkout's
file into a proof file. The same launch receipt replayed one run; tracking finished.
These are isolated profiles, not a deployment or a desktop/Android UI acceptance run.

Source: nightly4165 `8b44f42b14c8e273156474a924ccf29680dd7857`, confirmed
09:51 UTC. MCP launch/mode-limited replay, Scratch bootstrap, numeric Git checkout
percentages, first-message naming outside launch, full provider/project enable
precedence, latest live reference differential and compatible UI acceptance stay
open. The complete protocol goal also retains transfer/cross-instance handoff,
continuation settlement, newer native recovery and the published toolkit/auth/
environment/provider backlog. See MCP.md and RECOVERY.md.

Validation: full suites pass with 361 default / 358 no-default tests, 21 opt-ins ignored each. Final library, staged launch, storage, import/reader and handoff regressions are also checked after the last recovery/readiness changes. Strict Clippy runs in both configurations; fmt, generated compatibility and nine monitor tests pass.


### MCP launch, transaction ceilings and upload claims (2026-11-04)

MCP `t3_thread_launch` now calls the shared staged service. This supersedes earlier
MCP-launch pending notes; broader protocol limits remain. The source intake is still
nightly4165 SHA8b44f42b14c8e273156474a924ccf29680dd7857, confirmed at 10:37 UTC.

`tests/mcp_launch.rs` adds ten integration tests with real MCP HTTP and Connection:

- Effective project/global model defaults, configured exhausted-default fallback,
  caller/key replay after defaults disappear, options preservation and legacy forms.
- Explicit/inherited supported modes, narrowed headers and target-mode checks on replay.
- A trigger changes or revokes the grant at thread-claim commit: message/run admission
  fails, no preparation executes, and restoring authority retries the same identity.
- Project Git worktree membership (including stale/recreated paths), invalid local
  bases before claim, remote-only bases and Git commit-search preparation.
- Shared Scratch bootstrap, icon preservation, distinct folders/replay and typed RPC
  refusal when the server profile is inside a Git checkout.
- Pending upload copies, failed claim/message rollback and retry, deletion of pending
  originals before gated setup, and native consumption of the copied bytes by both
  Claude/Codex peers. Native caller sessions also invoke launch themselves: the new
  thread inherits project/model/modes and sender identity, defaults to project root
  despite the caller's worktree, and remains top-level rather than delegated.

`examples/live_mcp_launch.rs` passed with installed Claude Haiku and Codex gpt-6.1-sol.
Each native parent created one new top-level thread through MCP launch; model/modes/
project inherited, the synchronous setup gate prevented child provider admission,
and the child read a setup file and wrote its exact bytes into a proof file in the
new checkout. Both runs completed. These isolated live proofs do not certify the
compatible desktop/Android UI or external OAuth/linked routing.

Full suites pass with 371/368 tests and 21 opt-ins ignored per mode. Final affected MCP/staged/
handoff tests and strict Clippy pass in both configurations; fmt, generated table
and nine monitor tests pass. Non-MCP attachment intake, richer extraction, full provider/
project enable precedence, numeric checkout progress, transfer/continuation, remaining
toolkits/auth/environment/providers, newer runtime recovery and current differential/
UI acceptance remain part of the active goal. No deployment/main promotion.

### Shared attachment intake and document worker, 2026-11-04

PR26's exact head db56824 passed both CI modes and merged into dev e561df2.
The next port follows nightly4165 (`8b44f42b14c8e273156474a924ccf29680dd7857`),
confirmed again at 11:25 UTC. Ordinary messages, queued edits, runtime question
answers, staged launch and three new MCP attachment tools share pending-copy
admission outside SQLite's write lock. Scoped writes recheck authority at commit;
accepted receipts preserve their first copied identities and rejected attempts
remove unused copies. Native active sends retain sender and agent/MCP origin in
both the message and user-message item. Question answers expose original and
extracted paths with their source answer and attachment projection.

`tests/attachment_intake.rs` uses actual HTTP, the compatible lib-t3-rust client,
SQLite rejection injection and both native transport peers. It proves upload
ownership across transport sessions, native caller defaulting, canonical stored
metadata, unrelated-thread rejection, rollback and retry, queue-edit replay,
question sidecars and concurrent independent SQLite writes during gated extraction.
The native peers read the copied notebook and extracted text after pending discard.
`tests/document_text.rs` runs the real CLI against 64 evaluated nightly structure
fixtures plus RTF/notebook/Unicode cases and explicit archive/output/input bounds.
The parser covers all nine source formats. Unit tests exercise the isolated worker's
transient/permanent failure caches, private sidecars and output limits.

Installed Claude Haiku and Codex gpt-6.1-sol each completed
`examples/live_attachment_intake.rs`: an actual uploaded DOCX contained a generated
token unavailable in the prompt, the pending source was deleted, and the agent read
the extracted path and wrote the token exactly into its workspace. These are native
provider and compatible-client proofs for this flow. They do not certify full nightly
equivalence or current desktop/Android UI acceptance. Guest-sharing attachment
mediation and the rest of the protocol backlog remain pending. No deployment or
main promotion occurred. Source/validation inventory:
[attachment-intake.json](nightly/attachment-intake.json).

### Provider enablement, registry hydration and admission, 2026-11-04

PR27 passed both checks on dfae324 and merged into dev
002f26443e62d85ecaa132bc182ab2da043373be. The next port follows nightly4165,
confirmed at 11:53 UTC. Source instance defaults, envelope/config false precedence,
project override precedence and union hydration now drive the native registry and
shared message/launch admission. Omitted enable flags remain omitted when saved;
unknown ids do not inherit enabled state. Project-only enablement probes/executes
the configured native instance without changing its global setting. Other projects
remain refused, and accepted command replay survives a later disable.

`tests/provider_policy.rs` exercises real Claude/Codex adapter processes and the
compatible client, including its model catalog reader. It verifies lazy discovery,
settings-driven enable/disable/removal, rejected title generation while another
project keeps the registry entry enabled, unavailable-driver snapshots and default
probe exclusion/recovery after invalid JSON. `tests/mcp.rs` checks invalid_request
errors for disabled launch/plain/attachment sends with real HTTP and an uploaded
file. The policy matrix checks source driver defaults and all flag/override
combinations, plus model overrides and opaque-setting isolation. Scripted fixtures
now register their synthetic instance explicitly instead of depending on unknown
ids being enabled.

Full suites pass 388 default / 385 no-default, 21 opt-ins ignored each. This evidence
covers enablement, not full catalog/text-generation/driver equivalence. Current UI
acceptance, additional providers, history/managed-policy restoration, complete
text-generation fallback and the broader protocol backlog remain open. Source and
validation inventory: [provider-policy-intake.json](nightly/provider-policy-intake.json).
No deployment or main promotion.

## Context-transfer checkpoint (2026-11-04)

Pinned nightly4165 source drives ordinary fork/merge-back/side-chat plans and native
handoff delivery. `tests/context_transfer.rs` uses the compatible client and real
adapter subprocess peers for canonical boundaries, supersession/receipt replay,
metadata reset, merge guards, per-fork visibility, provider-changing merge in both
directions, side-chat digests, injection fallback and durable unknown-delivery refusal.
`tests/mcp.rs` checks status under read-only access and both write target ceilings.

The history fixture generator runs the pinned pure TypeScript functions. Seven
budgets match Rust message conversion, coverage, exact selection, rendered content,
response items and serialized cost. `tools/live-transfer-e2e` passed eight installed
Claude Haiku/Codex turns in isolated homes: private-codeword inheritance, historical
Codex anchoring after a later override, actual merge context consumption and latest
Claude side-chat framing with a preassigned native id. Native providers shut down
before temporary credential-copy homes are removed.

The command table explicitly marks fork/merge partial: Claude historical native
anchors, native-window budget integration, automatic uncertain-conversation replacement
and remaining provider-handoff/continuation parity are still open. See
[context-transfer-intake.json](nightly/context-transfer-intake.json). No deployment or
main promotion is part of this checkpoint.

Validation: full suites 403 default / 400 no-default, 22 opt-ins ignored each; final
affected suites 151 / 151 (6 ignored each), strict all-target Clippy both modes, fmt,
generated compatibility and nightly monitor (9 tests) pass. The eight-turn native opt-in
also passed explicitly.

## Native provider home checkpoint (2026-11-04)

PR31 passed both checks on fe2a063 and merged into dev
6f8257083009dc5b27782ccca4815cd5207b924f. The next port follows nightly4179,
confirmed through the official release API. The native runtime and continuation
keys now share Codex/Claude home resolution. Codex account overlays share history
and runtime state while preserving private authentication, model cache and local
runtime directories. Source-shaped conflicts reject existing real shared entries
and private auth symlinks; live local writer locks remain local until later setup.

Twenty-one generated cases execute pinned TypeScript resolvers. Filesystem tests
cover concurrent/idempotent setup, stale and relative links, runtime lock migration
and explicit error shapes. The compatible Rust client and subprocess peers cover
relative configured homes before workspace cwd changes, exact alias configuration,
shared native history across separate credential sentinels, pre-launch auth-symlink
refusal and recovery after explicit repair. No private credential is linked or copied
by the overlay materializer.

An installed Codex run found that unsubscribe can acknowledge before writer cache
eviction. Native sessions now request zero retention after unsubscribe and detach
waits for the native close notification under a deadline. A delayed-eviction peer
with OS writer locks covers this ordering. Three installed Codex inference turns
pass through the direct profile and two overlays: one native conversation, earlier
private codewords, separate regular auth files, no portable handoff or model tool use.
The proof uses private copies of one login; distinct account logins are not claimed.

Validation inventory: [provider-home-intake.json](nightly/provider-home-intake.json).
Complete selection/registry lifetime policy, active cross-instance lifecycle,
request_secret4179, broader provider/MCP/auth surfaces, current desktop/Android
acceptance and resource measurements remain open. No deployment or main promotion.

Validation: full suites 432 default / 429 no-default, 25 opt-ins ignored each;
strict all-target Clippy both modes, fmt, generated compatibility, monitor (9),
fixture reproduction and diff checks pass. The final default home integration
also passes with the sibling native process assertion.

## Selection transition checkpoint (2026-11-04)

PR32 passed both checks on 2cec3bd and merged into dev
0d6a6280380d139046a563e63aa116bc13dfd6d0 after the webhook fixture and
bounded parallel discovery repairs.

The next port follows nightly4179 source policy and its native adapter classifiers.
It distinguishes reuse, next-turn switching, restart/resume, portable handoff and
refusal; runtime/workspace changes do not override an adapter refusal or explicit
handoff. The planner selects the newest live session and retains negotiated
capabilities from dead records when native history remains. Metadata decisions
persist privately until native-owner admission, including same-instance classified
restart/handoff and queued restoration after unexecuted composer preferences.

Both native adapters accept same-instance preference changes while a turn is active.
Steering preserves that run/attempt and its original model/runtime policy; the next
turn captures the new preferences. Equivalent option reordering retains the native
process, MCP owner and approval policies. provider.switch uses the shared receipt
path. Legacy aliases use canonical driver/home/enable settings when no exact alias
exists, including a differently configured native driver under the canonical name.

The generator executes pinned TypeScript: 82 policy cases, 8 equality cases and 6 ACP
classifications. Service/compatible-client tests cover live/dead capability fallback,
corrupt-plan rollback, restart/handoff/refusal, State reopen, unavailable targets,
delete and native-owner queue restoration. Four installed Claude Haiku/Codex app
turns plus two active steering deliveries preserve a running shell command, one
original attempt and a private codeword in the same native conversation. Temporary
credential-copy homes and owned processes are cleaned on success/failure.

Full active cross-instance lifecycle, registry/pool cleanup, SDK options and source
background guards, exact exotic ICU collation ties, event/timeline fidelity and the
broader protocol/UI/resource backlog remain open. Evidence and validation inventory:
[selection-transition-intake.json](nightly/selection-transition-intake.json).

Validation: complete suites 441 default / 438 without default features, 26 opt-ins ignored each.
A final implicit restart preference regression was red, then fixed to use current
thread preferences; final restart/selection/compatible-client suites 28 per mode
(1 opt-in ignored), strict all-target Clippy both modes, fmt, generated compatibility,
9 monitor tests, 96-case fixture reproduction and diff checks pass. Four installed app
turns plus steering pass explicitly.

## Active provider transition checkpoint (2026-11-04)

PR33 passed both checks on 3d87dfc5eb424e6fd6af67b94efc11230c122ba9 and merged
into dev3aaaef3b9f2a48a693cf89f84e5d3cea734568a0. Its staged checkout cancellation
fixture now waits for durable cleanup acknowledgement as well as path removal.

The next adaptation follows nightly4179 active steering/restart policy. It compares
the incoming selection with the admitted run, preserves next-turn native steering,
and promotes an immediate provider/session change to interrupt-and-restart. The
run keeps its identity; the new attempt/root receives the incoming provider and
model. Compatible profiles resume their native history. Portable handoffs include
the current active ordinal and snapshot history before the replacement message.
Claude session binding preserves the staged attempt/root identity.

The restart journal waits for old native acknowledgement and execution release,
then detaches the exact outgoing native binding before registering the replacement
under the same run ID. Native release failures prevent delivery. Codex interrupted
command items can lack item/completed: tracked terminal IDs must pass explicit
termination/list confirmation before the adapter retires those command items.
Ordinary completed turns retain the existing owned-item drain.

Compatible-client regressions cover explicit restart, steer and auto, preference
changes, receipt rollback, delayed acknowledgement, stale old output, release
failure, physical terminal containment refusal and stop/delete/archive while
waiting. The installed-provider proof and final validation inventory are recorded
in [active-provider-handoff-intake.json](nightly/active-provider-handoff-intake.json).

Recursive native descendant containment, complete pooled/retired-session cleanup,
SDK options, source guards and exact event/modelSwitch/checkpoint-scope fidelity
remain open, alongside the broader MCP/auth/environment/drivers, differential,
desktop/Android and resource backlog. This is a partial protocol checkpoint.

Validation: final full suites447 default /444 without default features,27 opt-ins
ignored each; strict all-target Clippy both modes, fmt, generated compatibility,
9 monitor tests,96 pinned selection fixture reproduction and diff checks pass.
Four installed app turns prove both active replacement paths and private-history
recall. Native first-use shared-home SQLite initialization is serialized in that
proof; concurrent first-use initialization remains unverified.

## Provider scope and selected-binding checkpoint (2026-11-04)

PR34 passed both checks on d89b749182d531e57d66187dffb9c5c6cfd64110 and merged
into dev75fb4390e8f0d49502bc5a9da4f2e158e2676f2c. The next adaptation follows
nightly4179 ProviderInstanceRegistry scope replacement and ProviderSwitchService
release selection.

Modified and removed native instances retire before replacements admit native
work. Executions receive native interruption; metadata operations cancel. Closure
waits for admitted operations and native disposal, with cleanup owned independently
of the waiter. Builtin default scopes obey the same rule when overridden or restored;
other providers retain their processes. Scope snapshots prevent settings changes
from routing startup through an unmanaged default.

The private native-binding index retains the original owner key and binding
incarnation. Normal selection journals all planner-selected historical sessions,
preserving target peers and stopped/error records. Native ACKs mark only matching
bindings; a later rebind gets a fresh physical release receipt. Historical native
background work refuses direct selection atomically. Workspace planning uses live
session cwd before the target worktree path, as the source service does.

A controlled regression reproduced the fresh-home Codex SQLite failure: discovery
uses its own temporary app-server while the inference actor initializes the same
store. Startup now serializes initialize by shared home, including overlays; an
independent home can start while another is held. Cancellation retains that startup
gate until its child is reaped. Instance disposal also owns transient discovery and
quota actors, and confirms reaping when an initialize waiter disappears.

Four installed app turns with Codex CLI0.166.0 gpt-6.1-sol and Claude Haiku change
instance environment while a native shell runs. The old shell and CLI are reaped
before the replacement; the new process resumes native history and recalls a private
random codeword absent from its input. Native-peer tests additionally cover default
override/removal and unchanged neighbor processes. The final installed proof uses
the completed registry implementation; builtin routing is covered separately by
those process tests.

Full shared-session attachment/releaseUnlessBusy and queued background policy,
projection reconciliation on registry changes, credential/forced-rebuild epochs,
legacy owner reconstruction, recursive native lineage and the broader protocol,
differential/UI/resource backlog remain open. Evidence and final validation inventory:
[provider-pool-lifetime-intake.json](nightly/provider-pool-lifetime-intake.json).

Validation: full suites460 default /457 without default features,28 opt-ins
ignored each; strict all-target Clippy both modes, fmt, generated compatibility,
9 monitor tests,96 pinned selection fixture reproduction and diff checks pass.
The workspace service and concurrent startup regressions were observed red before
their production fixes; cancelled initialization verifies the exact child PID is
reaped before disposal acknowledges.

Nightly4192 (7f6f47b27b44456f1c8214d3b566b375c09f629e) appeared during final
validation. Registry, switch/session/orchestrator/selection sources and contract
schemas are unchanged from4179; inspected Codex adapter changes replace an
undefined-valued Effect with Effect.undefined. This checkpoint remains applicable
to those unchanged lifecycle sources. Broader4192 crypto/auth/PayloadSigner/DPoP
and stream-defect recovery changes require their own intake and verification.

## Nightly 4192 signature decoding checkpoint (2026-11-04)

PR35 passed both checks on fb8c09b672add3f4c3bfad9f8e18fee742cbf894 and merged
into dev 0070bf3. Nightly 4192 (7f6f47b27b44456f1c8214d3b566b375c09f629e) moves
server HMAC, key and random work from node:crypto/jose to effect/crypto behind
PayloadSigner and AssetSigner. Token layouts, claims, HMAC inputs and signing-key
names are byte-identical, so tokens issued by either server keep verifying.

The observable change is decoding. PayloadSigner and token payloads use Effect's
Base64Url: padded or unpadded input, CR/LF stripped, non-zero trailing bits
ignored. Tokens split as `const [payload, signature] = token.split(".")`, so later
segments are ignored. Webhook tags use Effect's Hex (ASCII hex digits only) and
Base64 (canonical padding, trailing bits ignored) before HMAC verification. o3's
session/websocket, asset/upload and webhook verifiers now follow those rules. The
decoder table is checked against effect@4.1.2 output, and the webhook regression
failed on the previous source because `"+f"` was accepted as a hex byte.

Desktop bootstrap, relay JWT/environment signing, DPoP and MCP OAuth refactors keep
their semantics; o3 does not serve those surfaces. The client-runtime stream-defect
fix changes no server wire. Evidence:
[signature-decoding-intake.json](nightly/signature-decoding-intake.json).

Validation: full suites 463 default / 460 without default features, 28 opt-ins
ignored each; strict all-target Clippy both modes, fmt, generated compatibility
and diff checks pass.

## Scheduled tasks for Android (2026-11-04)

Android acceptance on 2026-11-03 authenticated to both hosts, then failed with
`Unknown request tag: scheduledTasks.list`. That text is the defect of Effect's
RpcServer, so the replies came from T3 Node, not from o3, which answers unknown tags
with `o3-server does not implement …`. T3 nightly 3991 (live on NODE) and 4192 define
no `scheduledTasks.list`: their only task read is the `scheduledTasks.subscribe`
stream. lib-t3-rust had registered `list` from the older pinned contract.

o3 now serves `scheduledTasks.subscribe` like ScheduledTaskService.subscribeList.
It emits `{tasks}` at once, then the full newest-first list after every write, and
hides `webhook` without `orchestration:operate`. Upsert, setEnabled, delete and runNow
follow the nightly service: commandId-derived ids, `once` schedules, kept `nextRunAt`
for unchanged schedules, normalized webhook schedules with encoded hook paths,
idempotent delete, and webhook runNow refusal. `scheduledTasks.list` stays for older o3
clients. lib-t3-rust PR26 reads `scheduledTasks.list` from the stream, falling back
to the RPC only when the stream is unknown. Evidence:
[scheduled-tasks-intake.json](nightly/scheduled-tasks-intake.json).

Validation: full suites 466 default / 463 without default features, 29 opt-ins
ignored each; strict all-target Clippy both modes, fmt, generated compatibility and
diff checks pass. The opt-in differential against real T3 3991 and the existing signed
webhook differential pass.


## 2026-11-06 — Consume Claude's response after a background notification

The previous background test proved ownership until task completion, but accepted
settlement immediately on the last `task_notification`. It did not prove that the
parent's subsequent response reached the client. That was an execution parity gap.

A successful root result received while background work is pending only acknowledges
that root turn. When an owned task subsequently notifies completion, the adapter now
invalidates that earlier success and keeps reading until Claude finishes its response
to the notification. Duplicate or unrelated notifications do not invalidate a result.
An interrupt still settles from its existing acknowledgement without requiring another
native result. A zero-turn notification result is accepted only while consuming an
owned continuation; an unrelated notification cannot finish a newly submitted prompt.

Evidence: the real-client regression gates the parent's result independently from task
completion and requires the parent's streamed text while the app run is still running.
It failed against the previous adapter. The strengthened installed-Claude Haiku proof
also failed before the fix (only `STARTED` survived). After the fix, a background Bash
command exits with code 7, Claude receives its failure notification, runs a second Bash
command that writes `RECOVERY_OK`, and replies `AFTER_NOTIFICATION` before app completion.
The protocol-client background, legacy identity, restart and interrupt cases pass.

This corrects premature settlement during an owned run. It does not establish complete
idle-session wake routing or native goal projection parity, and does not replay failed
commands or infer that all provider errors are safe to retry.


## 2026-11-06 — Provider-native goals and owned automatic turns

Reference: nightly4262, `c55db3e9c72fdf584f4b54192fb39beb88667b59`.
The Codex adapter previously submitted `/goal` as ordinary inference text and
closed a successful native turn without owning the next automatic goal turn.

Control-only commands now use native goal APIs and synthetic provider turns with
no native rollback reference. Set validates the 4000-character Unicode objective
limit before changing the current goal, installs it paused, starts with the run's
captured execution settings, and activates only afterward. Resume uses empty native
input. Goal turns remain one app run while their provider turns and messages close
separately. The five-second continuation grace pauses a goal before releasing its
run. Stop can proceed while activation is pending; later acknowledgements and native
orphan turns are fenced and stopped. Request responses cannot overwrite newer goal
notifications. Explicit user provenance is kept for Codex0.167 compatibility.

Claude acknowledgements and synthetic Stop-hook feedback update the goal projection.
A successful checked root can complete the goal. Child output, unverified command
acknowledgements, failed turns and non-completion terminal reasons cannot do so.

`tools/generate-native-goal-fixtures.mjs` executes 49 cases through the pinned
upstream parser and goal normalizers. `tests/native_goals.rs` drives the real
Connection through control commands, fast completion, automatic continuation,
Stop/activation races, late orphan starts, cleared goals, resume, rollback references,
Unicode objective limits and failed replacements. Installed Codex0.166/0.167 and
Claude Haiku have separate isolated-state proofs. The installed Codex continuation
proof requires PHASE_ONE and PHASE_TWO in separate native turns within one app run,
with terminal assistant messages and a completed native goal.

Exact validation and remaining limits: [native-goal-intake.json](nightly/native-goal-intake.json).

### Resume notifications and activation ordering

Installed Codex 0.167 omits goals from `thread/resume`'s thread object and can emit
`thread/goal/updated` separately while resuming an active objective. A private
offline container probe preserved the objective and token budget across a native
process restart, confirming that the notification is authoritative. It used an
empty native home, no credentials and no network; no successful inference was
required.

The actor now retains normalized goal notifications before subscription, including
explicit clears. The cache has 512 entries, bounds native IDs and objectives,
rejects malformed budgets and protects currently subscribed entries from eviction.
RPC snapshots remain fenced by the notification version. The provider publishes
the observed state after binding and uses it for automatic-turn ownership. Unknown
native children do not become owned root conversations.

Three compatible-client regressions cover a missing legacy goal on resumed active
work, a native clear overriding stale active state, and an unrelated child whose
goal must neither be adopted nor paused. The active-resume regression fails on
the preceding implementation: its app run completes while its goal remains null.
All three assert that ordinary messages issue no native goal-control RPCs.

A separate actor regression stalls the consumer until activation succeeds, then
checks the preparation notification, fast first-turn completion, active update
and acknowledgement in exact wire order. Successful activation enters the same
bounded route; its oneshot reports errors and deadlines without applying success
out of order. The internal acknowledgement never reaches the public event stream.

This evidence covers the subscription and activation boundaries. It does not
certify the complete idle-wake, automatic-turn recovery or cross-process handover
lifecycle, and introduces no deployment.
This block is not a complete headless parity certificate; [PARITY.md](PARITY.md)
includes every headless surface, methods without a current client caller, the new
RPCs and the migration80 acceptance backlog.
## 2026-11-06 — Nightly thread reads

`tests/thread_reads.rs` drives title batches, thread families and worktree membership
through the real compatible client. Archived threads retain titles; deleted and unknown
IDs do not. Family selection accepts deleted anchors and includes active forks,
subagent descendants and side chats, ordered by update time and ID. Worktree selection
uses the exact stored path. Family summaries and their replay boundary use one SQLite
read transaction. All three methods require `orchestration:read`.

The opt-in `official_nightly_matches_thread_read_membership_order_and_titles` runs
against the official 4262 headless release in rootless Podman with no network,
credentials or writable host mounts. Twelve valid query comparisons and 31 malformed
input comparisons agree, including complete `Die` exit frames and Unicode whitespace.
Nine additional comparisons verify scope/decode ordering and permission denials,
including the upstream legacy `requiredScope` and granular `requiredPermission` pair.
Regular tests also cover 35000-ID title batches, denied scopes and wire defects.

Run `tools/test-headless-reference /path/to/extracted-nightly` to reproduce the native
comparison. It compiles on the host and executes the proof in the container. This is
evidence for the queries' selection, ordering, input decoding and authorization;
complete common shell summaries and source-wide error taxonomy remain open.

See `nightly/thread-reads-intake.json` and `PARITY.md` for current scope.
## 2026-11-06 — Personal GitHub star operations

`src/forge.rs` and `tests/forge.rs` cover star reads and mutations with real HTTP
and the compatible RPC client. A private loopback fixture proves read/404, PUT/204
and read/204 without external mutations. Only HTTP 204 succeeds. Header, quota reserve,
credential boundary and pause cases use actual requests. Local port-discovery probes
receive 404 without consuming a prepared forge response.

Eighty-three cases execute the upstream's host, path, API-root, environment-token and
private-slot naming helpers. Twenty-two comparisons with the official 4262 runtime
cover unsupported/disabled cases, malformed-input wire defects and read-only scopes;
their source is unchanged in 4275. Native token placeholders load from T3's private
`.bin` slot, keyed literals are redacted, and settings round trips preserve them.

GitHub App sign-in, secret-provider resolution, source-wide secret persistence and
credential/quota integration with other forge consumers are still partial. No real
GitHub repository is starred by these tests. Evidence and remaining limits live in
`nightly/forge-star-intake.json`; this block does not certify full headless parity.
