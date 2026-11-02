# Conformance evidence

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
