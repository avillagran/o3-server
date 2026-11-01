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
worktree directory; cleanup invocation is also exercised. The final reader sweep
keeps `storageCleanup` and `projectWorktreeCleanup` explicitly false: their policies
only round trip and manual cleanup only removes old logs. `storageCleanupRun` and
`worktreesDirectory` are true. Policy/worktree collection remains a consumed gap;
config/settings rows remain partial.

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
