# Conformance evidence

This is a partial operational checkpoint, not a full interchangeability certificate.
The owner reprioritized startup memory and P0 operation; unresolved work is listed below.

Reproduce with `tools/conformance-diff` (nice, cargo -j 4, no bridge feature). It starts
one fresh T3 0.1.7-nightly.20261031.3991 and one fresh o3 on independent free loopback
ports under `/var/tmp/o3-build/tmp-diff`. `T3_REFERENCE_BIN` can select another binary.
`HOME`, XDG roots, provider environment and `T3CODE_HOME` are isolated. Node's actual
runtime path is used instead of a HOME-sensitive mise shim. The user's running T3,
profile and service are never used. Servers have private process groups; normal exit
terminates them and removes profile/workspace state. Reports retain shapes only.
No inference credentials are imported. Builds use `/var/tmp/o3-build/diff`.

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
The pinned source is `/home/node/Work/t3-reference-3726`, contracts at 3f9448747.

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

Accepted differences (row promoted):
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
- terminal.open/attach: scrollback is not restored after a server restart.
- thread.user-input.dismiss: native delivery to the provider and recovery are unfinished (`lib-t3-rust/src/questions.rs:48`).
- thread.fork, thread.model-selection.set: native resume-at mapping and cross-provider handoff.
- updateSettings, vcs.refreshStatus: settings normalization; GitLab/self-hosted change requests.
