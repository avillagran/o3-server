# Client usage inventory: what the O3 clients actually ask of a T3 server

Purpose: scope `o3-server`, a lean Rust replacement for the T3 Code headless server. It implements only what our
clients touch at runtime. Sources read on 2026-10-31 (read-only):

| Short | Path | What |
|---|---|---|
| `D/` | `/home/node/Work/omarchy-t3-code` | O3 desktop (Rust backend `omarchy-t3-client` + QML). Consumes `t3-protocol` vendored at `D/vendor/t3-protocol` (same source as L). |
| `A/` | `/home/node/Work/ot3-android` | O3 Android (Kotlin). Consumes the lib via `t3-lib` AAR (JNI → `T3Client`). Core code in `A/core/src/main/kotlin/dev/avillagran/ot3android/core/` (`AC/`), UI in `A/app/src/main/kotlin/dev/avillagran/ot3android/` (`AA/`). |
| `L/` | `/home/node/Work/lib-t3-rust` | The lib: `src/api/*` (typed RPCs, 105 registry rows + 13 streams), `src/wire.rs`, `src/stream.rs`, `src/lib.rs` (`Connection`), `tools/extract-t3-contract`. Pinned to `t3code@3f9448747` (`L/src/api/mod.rs` `CONTRACT`). Ground-truth shapes: `L/tests/fixtures/t3-contract.json`. |

Legend. **P0** = client cannot connect, or cannot do core chat. **P1** = daily feature. **P2** = rare, settings, admin.
**unused** = wrapped in the lib, no client calls it. "D" = desktop, "A" = Android. A cell reads "yes" only when a
call site is cited; "lib" means only the lib calls it internally (still needed on the wire).

How "invoked" was decided. Desktop: grep of every `module::fn`, `Function` const, `api::call/function(tag)` and raw
`c.request_json`/`Client::get` in `D/src`. Android: grep of every `transport.call("…")`, `client.call`, `subscribeFn`,
domain "recipe" results (`method` fields) in `A/app|core/src/main`. Test-only and `sharedTest` fixtures are excluded
from "used".

---

## 0. Wire basics the server must speak (all P0)

| Surface | Detail | Used by | Evidence |
|---|---|---|---|
| `GET /.well-known/t3/environment` | No auth. JSON descriptor. Must contain non-empty `environmentId` (uuid; `pairBearer` additionally enforces uuid). Android shows `label`, `serverVersion`, `platform.os`, `orchestrationProtocolVersion`. Desktop reads `environmentId` to check identity of SSH/OmaBridge-forwarded hosts and as `reachable()` probe. | D, A | L `src/pairing.rs:7,41-52`; A `AC/connection/ServerProbe.kt:13-40,77-90`; A `AC/connection/ConnectionSupervisor.kt:304`; D `src/reconnect.rs:23`, `src/main.rs:196,411,437`, `src/connection_access.rs:61,112` |
| `POST /oauth/token` (form) | RFC 8693 token-exchange. `grant_type=urn:ietf:params:oauth:grant-type:token-exchange`, `subject_token=<pairing secret>`, `subject_token_type=urn:t3:params:oauth:token-type:environment-bootstrap`, `requested_token_type=<ACCESS const in L>`, optional `client_label/client_device_type/client_os`. Response `{access_token, token_type:"Bearer", scope, expires_in}`; anything but `Bearer` is rejected. Android pairing flow only (desktop local uses a CLI-minted token, §9). | A (D lib-only) | L `src/pairing.rs:123-186`; A `AC/pairing/PairingService.kt:163-188`, `AC/transport/NativeT3Transport.kt:109-121` |
| `WS /ws?orchestrationProtocol=2` | Effect-RPC JSON text frames. Auth by `Authorization: Bearer <token>` upgrade header (DPoP credentials use `?wsTicket=` from `POST /api/auth/websocket-ticket`, only for T3 Connect relay). Client → server: `{"_tag":"Request","id":"1","tag":<rpc>,"payload":{…},"headers":[]}`, `{"_tag":"Ping"}`, `{"_tag":"Interrupt","requestId":"1"}`, `{"_tag":"Pong"}`. Server → client: `{"_tag":"Exit","requestId":"1","exit":{"_tag":"Success","value":…}}` or `exit:{"_tag":"Failure","cause":…}`; streams: `{"_tag":"Chunk","requestId":"1","values":[…]}` then (on end) `Exit`. WS-level Ping/Pong also answered. **Unary RPC = one socket per call** (`id` is always `"1"`), so the server must tolerate a high connection churn. Stream liveness: client pings every 5 s, drops after 20 s of silence. Max message 8 MiB (64 MiB for bounded thread snapshots). | D, A | L `src/lib.rs:508-565` (`websocket`), `:566-595` (`rpc`, 20 s deadline), `src/stream.rs:1-128`; error classification `L/src/errors.rs` (`EnvironmentAuthorizationError` + `requiredScope`) |
| Auth scopes | Bearer token carries scopes: `orchestration:read`, `orchestration:operate`, `filesystem:read`, `settings:write`, `providers:manage`, `terminal:read`, `terminal:operate`, `preview:operate`, `source-control:write`, `diagnostics:read`, `environment:maintain`, `access:write`. Desktop local token is parsed client-side: `payload.signature` or JWT-like, claims `scopes[]`, `exp` in **ms**. The server must therefore emit tokens in that format. | D | D `src/connections.rs:139-160` (`token_expiry_ms`, `has_scope`) |
| `GET /api/auth/session` | `{authenticated:bool, permissions|scopes:[…]}`. The clients derive `canWrite` (`orchestration:operate`), `canShareGuest` (`access:write`), terminal permission (`terminal:operate`), providers writable (`providers:manage`), and the Android maintenance page action gating from it. | D (≥14 call sites), A | D `src/write.rs:66-68,436`, `src/provider_instances.rs:8`, `src/keybinding_settings.rs:15`, `src/quota_sources.rs:8`; A `AC/transport/NativeT3Transport.kt:49`, `AC/terminal/TerminalBindings.kt:11-30` |
| Header `x-t3-orchestration-protocol: 2` | Sent on protocol GETs (`shell`, `bounded`, `history`) when the client passes `protocol=true`. | D, A | L `src/lib.rs:499` |

---

## 1. Connection, pairing, identity

| Surface | P | D | A | Call sites / notes |
|---|---|---|---|---|
| `GET /.well-known/t3/environment` | **P0** | yes | yes | see §0 |
| `POST /oauth/token` | **P0** (A) | lib | yes | §0. Pairing deep link / code → `PairingService`. |
| `GET /api/auth/session` | **P0** | yes | yes | §0 |
| `GET /api/auth/clients` | P2 | yes | lib | D `src/connection_access.rs:66` (access page); L `src/pairing.rs:195` (`revoke_stale_sessions`, desktop prunes its own stale sessions at startup, label subject `omarchy-t3-code:<16hex>`) |
| `POST /api/auth/clients/revoke` `{sessionId}` | P2 | yes | lib | D `src/connection_access.rs:105`; L `src/pairing.rs:203` |
| `POST /api/auth/clients/revoke-others` | P2 | yes | no | D `src/connection_access.rs:105` |
| `GET /api/auth/pairing-links` | P2 | yes | no | D `src/connection_access.rs:67` |
| `POST /api/auth/pairing-token` | P2 | yes | no | D `src/connection_access.rs:101` (creates a code others use at `/oauth/token`; **needed so Android can pair to an o3-server** unless pairing is created some other way: `t3 auth pairing create` CLI or the `/pair` URL, see §9) |
| `POST /api/auth/pairing-links/revoke` `{id}` | P2 | yes | no | D `src/connection_access.rs:105` |
| `POST /api/auth/websocket-ticket` | P2 | lib (device hub, relay only) | no | L `src/device_hub.rs:78`, `src/lib.rs:519` (DPoP creds only). Bearer paths never call it. |
| `GET /api/connect/link-state`, `POST /api/connect/preferences`, `POST /api/connect/reachability` | P2 / optional | yes | no | D `src/connection_access.rs:70,119`, `src/nightly.rs:41`. T3 Connect (cloud relay) pieces; o3-server can return 404 → desktop shows `connectError`. |
| T3 Connect relay (`/v1/client/dpop-token`, `/v1/environments/{id}/connect`) | optional | yes | yes | Hosted by T3's relay, not by the server. Only relevant if o3-server wants to be reachable via relay. L `src/lib.rs:660-760`; A `ConnectRoute.Relay` `AC/transport/NativeT3Transport.kt:41-43`. Not P0. |
| `GET /api/projects` (non-protocol) | P2 | yes | no | D `src/connection_route.rs:25`, `src/journal_vault.rs:1548,1701` (project list for load balancing and the dev vault). Returns `{projects:[…]}`. Not in the lib. |

## 2. Shell (sidebar) and thread reads/streams

| Surface | P | D | A | Call sites / notes |
|---|---|---|---|---|
| `GET /api/orchestration/shell` (+`protocol` header; optional query `omitIdleSubagents=1`, `omitOlderSettled=1`) | **P0** | yes | yes | A first-run + reseed `AC/connection/ConnectionSupervisor.kt:309`, `AC/transport/NativeT3Transport.kt:106,224`; D `src/nightly.rs:3-13` (the shell read; ~12 other callers: `branch_picker.rs:9`, `git_controls.rs:14`, `suggestions.rs:12`, `terminal.rs:59`, `scoped_settings.rs:28`, `working_review.rs:8`, `disk_space.rs:109`, `thread_details.rs:10`, `archive_management.rs:161`, `desktop_cloud.rs:308`) |
| WS `orchestration.subscribeShell` `{afterSequence?, requestCompletionMarker?, omitIdleSubagents?, omitOlderSettled?}` | **P0** | yes | yes | D `src/shell_watch.rs:71-90`, cursor `src/shell_state.rs:51,171`; A `AC/connection/ConnectionSupervisor.kt:322` (`ThreadStream.shell`), resume cursor persisted. Items: `{kind:"snapshot",snapshot:{snapshotSequence,…}}`, then `{kind:"event"|…,sequence,…}`; server replays ≤1000 events after `afterSequence` else sends a fresh snapshot. Shell events reduced: `project.updated/removed`, `thread.updated/removed` (A `AC/shell/ShellReducer.kt:125-144`). |
| WS `orchestration.subscribeThread` `{threadId, afterSequence?, requestCompletionMarker?, acceptBoundedSnapshot, acceptCompactTurnItems, acceptDedupedBodies, acceptCompactIds}` | **P0** | yes | yes | D `src/live.rs:38-41` (sends all four accept flags `true`); A `AC/thread/ThreadSession.kt:278` (`boundedSnapshot=true,compactTurnItems=false`). "Nightly 3726" wire opt-ins: compact ids table + deduped bodies (`L/src/wire.rs`, `L/docs/NIGHTLY_3726.md`). A server may ignore the `accept*` flags and send plain items. The thread cursor: snapshot `snapshotSequence`, events `sequence`. |
| `GET /api/orchestration/threads/{id}/bounded` (+protocol header) | **P0** | yes | yes | D `src/live.rs:23` → lib `Connection::bounded_thread`; `src/lib.rs:430`; A `AC/transport/NativeT3Transport.kt:132-134` (`boundedThread`), `ThreadViewModel.kt:693,720`. Response `{projection:{thread,messages,runs,turnItems,…}, snapshotSequence, …}`; limit 64 MiB. |
| `GET /api/orchestration/threads/{id}/history?cursor=<opaque>&compactIds=1` (+protocol header) | P1 | yes | yes | D `src/write.rs:99-103` (`history`); A `AC/transport/NativeT3Transport.kt:76`, `AA/ui/thread/ThreadViewModel.kt:276`. Response `{items[], hasMoreHistory:bool, nextCursor:string|null}`; older-messages paging. |
| `orchestration.getThreadProjection` | P2 | yes | no | D `src/checkpoint_operations.rs:22` (rewind preflight) |
| `orchestration.getTurnItem` | P1 | yes | no (tests only) | D `src/failure_context.rs:12`, tool inspector (`D/src/tool_inspector.rs` via `toolInspector.request` recipe → this tag; A also builds it `AA/ui/thread/ThreadViewModel.kt:748-752`: recipe `method` is whatever `L/src/tool_inspector.rs` names, i.e. `orchestration.getTurnItem`) |
| `orchestration.getTurnDiff` | P1 | yes | yes | D `src/diff_review.rs:18-20`; A `AC/thread/ThreadDiff.kt:28` |
| `orchestration.getFullThreadDiff` | P1 | yes | no | D `src/diff_review.rs:18-20` |
| `orchestration.searchThreads` | P1 | yes | yes | D `src/thread_search.rs:22`; A `AC/home/MessageSearch.kt:39`, `AA/ui/home/HomeViewModel.kt:246` |
| `orchestration.getArchivedShellSnapshot` | P2 | yes | yes | D `src/archive_management.rs:150`; A `AC/maintenance/MaintenanceController.kt:16` |
| `orchestration.getOlderSettledThreads`, `getThreadFamily`, `getWorktreeThreadIds` | P2, capability-gated | yes | no | D `src/nightly.rs:21-23` (actions `older-settled|older-counts`, `family`, `worktree-threads`). Only reachable when `server.getConfig` advertises `shellOlderSettled` / `shellThreadFamilies`. o3-server can leave those flags false. |
| `orchestration.getWorkflowScript` | P2 | yes | no | D `src/write.rs:408` |
| `orchestration.stopWorkflow` | P2 | yes | no | D `src/write.rs:409` |
| WS `orchestration.subscribeCommandOutput` `{threadId,itemId}` | P1 | yes | yes | D `src/command_output.rs:80`; A `AC/transport/NativeT3Transport.kt:91-103` (live tool output; first item `{kind:"replace"}`) |

## 3. Commands (mutation path) and thread lifecycle

All writes are `orchestration.dispatchCommand` with a `commandId` (idempotent: **the server must dedupe by `commandId`**; both clients resend the byte-identical payload after `rpc_outcome_unknown`) or `orchestration.launchThread`. Response contains at least `sequence` (Android reads `response.sequence`; `launchThread` must echo `threadId`, A `AC/thread/MessageSender.kt:191-196`).

| Surface | P | D | A | Call sites / notes |
|---|---|---|---|---|
| `orchestration.dispatchCommand` | **P0** | yes | yes | D `src/write.rs:18,284` (`manage`/`dispatch`); A `AC/thread/MessageSender.kt:190,218`; `AA/ui/settings/maintenance/MaintenanceScreen.kt:65` |
| `orchestration.launchThread` (new thread + first message, optional worktree) | **P0** | yes | yes | D `src/write.rs:27,289`; A `AC/thread/MessageSender.kt:191` (`_ot3aLaunchThread` outbox envelope) |
| command `message.dispatch` | **P0** | yes | yes | D `src/write.rs:186` (`dispatchMode` `start_immediately`/`queue_after_active`, `deliveryIntent` `auto`/`steer`, `creationSource:"web"`, `attachments[]`, optional `modelSelection`, `runtimeMode`, `interactionMode`, `guestMessageId`, `sourcePlanRef`), `src/nightly.rs:36`, lib compaction `L/src/compaction.rs:154`; A `AC/thread/MessageSender.kt:254-270` |
| command `run.interrupt` (`runId`, `holdQueue:true`) | **P0** | yes | yes | D `src/write.rs:181`; A `AC/thread/MessageSender.kt:167` |
| command `thread.create` | **P0** | yes | no | D `src/write.rs:93`, `src/pull_request_actions.rs:40` |
| command `runtime-request.respond` (approvals + question answers) | **P0** | yes | yes | D `src/write.rs:163-175` → `L/src/approvals.rs:93`, `L/src/questions.rs:71`; A `AC/thread/MessageSender.kt:65` (`"approvals.command"` envelope, built by the lib domain). Needed for any agent tool approval. |
| command `thread.user-input.dismiss` | P1 | yes | no | `L/src/questions.rs:48` |
| commands `queued-run.cancel`, `queued-run.reorder`, `queued-message.promote-to-steer`, `queue.resume`, queue edit | P1 | yes | yes | `L/src/queue.rs:36-40`; D `src/write.rs:177`; A `AC/thread/MessageSender.kt:57` (`"queue.command"` envelope) |
| `thread.archive`, `thread.unarchive`, `thread.delete`, `thread.pin`, `thread.unpin`, `thread.pin.reorder`, `thread.active.reorder`, `thread.settle`, `thread.unsettle`, `thread.snooze`, `thread.unsnooze`, `thread.mark-unread`, `thread.auto-settle.set` | P1 | yes | yes | D sidebar builders `L/src/sidebar.rs:68`, `D/src/write.rs:471-478` (`pin_reorder`), `D/src/archive_management.rs`; A `AC/home/ThreadManagement.kt`, `AC/home/ThreadActions.kt`, `AA/ui/settings/maintenance/MaintenanceScreen.kt:64` (unarchive/delete), domain recipes `sidebar.pinCommands`, `sidebar.pinPlan` |
| `thread.metadata.update` (rename, branch, `limitRecovery`) | P1 | yes | yes | D `src/branch_picker.rs:113`, `src/nightly.rs:33`; A thread rename via domain recipe |
| `thread.visit` (watermark) | P1 | yes | no | D `src/write.rs:425` |
| `thread.fork` | P1 | yes | no | D `src/write.rs:263` |
| `thread.runtime-mode.set`, `thread.interaction-mode.set`, `thread.model-selection.set` | P1 | yes | yes | `L/src/models.rs:10`; D `src/write.rs:49-60`; A `NativeParity.kt` domain `models.*` |
| `checkpoint.rollback` (rewind) | P2 | yes | yes | `L/src/checkpoint_revert.rs:175`; D `src/checkpoint_operations.rs`; A `AC/git/CheckpointRollback.kt:54` |
| `subagent.stop` | P2 | yes | no | D `src/write.rs:275` |
| `project.create`, `project.update`, `project.delete` (via `projects.mutate`, see §5) | P1 | yes | yes (domain `projects.mutation`) | D `src/write.rs:255`; A `AC/transport/NativeParity.kt:14` |
| `thread.pull-request.link/unlink/sync/watch`, `thread.pull-request-link.sync` | P2 | yes | yes | `L/src/pull_requests.rs`, A `PullRequestWrites.kt:40` (`pullRequests.threadCommand`) |
| `thread.guest-share.close`, `thread.guest-message.dismiss` | P2, nightly | yes | no | D `src/nightly.rs:34-35` |
| `provider.switch`, `thread.stop`, `thread.title.regeneration.complete`, `thread.created.record`, `notification.delivery.accept`, `thread.auto-settle` | unused (lib builders only) | no | no | In `L/src/api/orchestration.rs` command enum; no client call site found |

Persisted outbox on both clients (Android Room `OutboxDao`, desktop `checkpoint_journal`/`drafts`): clients tolerate `rpc_outcome_unknown` by resending; a server without `commandId` dedup would double-send messages.

## 4. Server config and settings

| Surface | P | D | A | Call sites |
|---|---|---|---|---|
| `server.getConfig` | **P0** | yes (25) | yes | D `src/write.rs:54` (`catalog`), `src/auto_compact.rs:56`, `src/connection_route.rs:12`, `src/server_capabilities.rs:34`; A `AA/ui/newchat/NewChatDraftViewModel.kt:77`, `AA/ui/thread/ThreadViewModel.kt:392,648`, `AC/home/ThreadManagement.kt:11`, `AC/providers/ProviderAdministration.kt:23` |
| WS `server.subscribeServerConfig` `{usageLimitSources?:true}` | P1 | yes | yes | D `src/quota_sources.rs:28`, `src/keybinding_settings.rs:39`; A `AC/transport/NativeT3Transport.kt:150`, `AC/usage/QuotaReader.kt:26`. First item `{type:"snapshot",…}`. |
| `server.getSettings` | P1 | yes (13) | yes | D `src/write.rs:106,119`, `src/integration_devices.rs:285`; A `AA/ui/settings/projects/ProjectConfiguration.kt:39`, `AC/settings/ScopedSettings.kt:43` |
| `server.updateSettings` `{patch, providerInstanceMutation?}` | P1 | yes (7) | yes | D `src/write.rs:21,109`; A `AA/ui/thread/ThreadViewModel.kt:432`, `AC/settings/ScopedSettings.kt:61` |
| `server.discoverSourceControl` | P1 | yes | no | D `src/write.rs:74` |
| `server.reportClientActivity` | P1 | yes | no (lib only) | D `src/main.rs:177` (visible/focused heartbeat; drives notification routing server-side). A: `L/src/api/server.rs` wrapper, no call. |
| `server.upsertKeybinding`, `server.removeKeybinding` | P2 | yes | no | D `src/keybinding_settings.rs:233,235` |
| `server.searchAcpRegistry`, `server.prepareAcpRegistryAgent` | P2 | yes | no | D `src/acp_registry.rs:10,16` |
| `server.refreshProviders` | P1 | yes | yes | D `src/provider_login.rs:60`; A `AA/ui/usage/ProviderRefreshGate.kt:18`, `AC/providers/ProviderAdministration.kt:22` |
| `server.updateProvider` | P2 | yes | yes | D `src/provider_updates.rs:46`; A `AC/providers/ProviderAdministration.kt:50` |
| `server.getUsageSummary`, `server.refreshUsageRates` | P2 | yes | yes | D `src/usage_summary.rs:24,34`; A `AC/usage/UsageReader.kt:13`, `AA/ui/usage/UsageViewModel.kt:38` |
| Config capability flags (read from `getConfig`) | **P0 to define** | yes | yes | `shellResumeCompletionMarker`, `threadResumeCompletionMarker`, `shellThreadFamilies`, `shellOlderSettled` (D `src/server_capabilities.rs:19-23`); `guestSharingEnabled`, `settings.environmentIcon`, `environment.{serverVersion,platform.machine,capabilities.projectCloneTracking}`, `newProjectsRoot`, `settings.addProjectBaseDirectory`, `providers[]` (instanceId, driver, enabled, installed, status, auth.status, models[], slashCommands, setup.canAuthenticate, `showInteractionModeToggle`, `supportsConversationRollback`), `serverBrowser` (D `docs/BROWSER_RUNTIME.md`). Missing flag = feature off, so o3-server can start with all false. |

## 5. Projects, files, workspace

| Surface | P | D | A | Call sites |
|---|---|---|---|---|
| `projects.readFile` `{cwd, relativePath}` | P1 | yes | yes | D `src/documents.rs:18`; A `AC/theme/EnvironmentThemeReader.kt:121`, `AC/settings/ScopedSettings.kt:51` |
| `projects.searchEntries` | P1 | yes | no | D `src/suggestions.rs:22` (composer `@file` suggestions) |
| `projects.mutate` (project.create/update/delete) | P1 | yes | domain-built | D `src/write.rs:288`, `src/agent_sessions.rs:22` |
| `projects.createNew`, `projectClone.start` | P2 | yes | no | D `src/write.rs:29,30` |
| `projects.listIgnoredFiles` | P2, nightly | yes | no | D `src/nightly.rs:24` |
| `filesystem.browse` (folder picker) | P1 | yes | yes | D `src/write.rs:368`; A `AC/theme/EnvironmentThemeReader.kt:107` |
| `filesystem.getMetadata` | P2 | yes | yes | D `src/journal_vault.rs:200`; A `AC/theme/EnvironmentThemeReader.kt:101` |
| `shell.openInEditor` | P2 | yes | no | D `src/keybinding_settings.rs:278`, `src/thread_details.rs` |
| `review.getDiffPreview`, `review.getDiffFileContents` | P1 | yes | yes / no | D `src/working_review.rs:37`, `src/review_document.rs:183`; A `AC/git/GitReviewReader.kt:21` |
| `worktreeSetup.cancel` + WS `subscribeWorktreeSetup` | P2 | yes | no | D `src/write.rs:406`, `src/worktree_setup.rs:22` |

## 6. Attachments and assets (HTTP data plane)

| Surface | P | D | A | Call sites / notes |
|---|---|---|---|---|
| RPC `attachments.createUploadUrl` `{type:"image"\|"file",name,mimeType,sizeBytes}` → `{attachmentId, relativeUrl:"/api/attachments/upload/<id>"}` | P1 | lib | lib | L `src/lib.rs:405` (inside `Connection::upload_attachment`); D `src/write.rs:107-123` (`upload`), `src/clipboard.rs:29`; A `AA/AppGraph.kt:275` (`nativeClient.upload(fd,…)`), `ThreadViewModel.kt:182` |
| `POST /api/attachments/upload/{attachmentId}` (body = raw bytes, `Content-Type: mime`, 2xx = ok; **no credential header**, URL is the grant) | P1 | lib | lib | L `src/lib.rs:412-436`. Limits enforced client-side: images ≤10 MiB, files ≤50 MiB, ≤100 attachments, total image ≤80 MiB (D `src/write.rs:123-150`). |
| `attachments.delete` | P2 | yes | no | D `src/write.rs:427` |
| RPC `assets.createUrl` `{resource:{_tag:…}}` → `{relativeUrl}` | P1 | lib | lib | resources used: `attachment` (D `src/lib.rs:84`, A `NativeT3Transport.kt:68-71`), `workspace-file`/`media-file` (A `:73-80` `mediaImage`; D `src/images.rs`, `src/documents.rs:14-18`, `src/markdown_images.rs`), `project-favicon` (`L/src/assets.rs:96`), `tool-output-image`, `draft-workspace-file`, `native-app-icon`, `github-media` (lib wrappers; D sidebar icons). Scopes: `filesystem:read` for file resources, `orchestration:read` otherwise (`L/src/api/assets.rs:11-60`). |
| `GET /api/assets/{…}` (signed, unauthenticated, same origin, path **must** start `/api/assets/`; `Content-Disposition` inline for attachment/media; project-favicon "missing" sentinel suffix `FAVICON_MISSING`) | P1 | lib | lib | L `src/assets.rs:113-148`, `src/lib.rs:861-870`, `src/documents.rs:17` |
| ACP registry SVG icons (`https://cdn.agentclientprotocol.com/registry/v1/*.svg`) | n/a | lib | lib | Public CDN, not the server. `L/src/registry_icon.rs`. |
| `POST /api/pull-requests/diff` | P2 | yes | yes | D `src/write.rs:360`; L `src/pull_requests.rs:389`; A `NativeT3Transport.kt:90-92` (`pullRequestDiff`). Body from `pullRequests.request` domain recipe (method `diff`), 8 MiB cap. |

## 7. Git / VCS / pull requests

| Surface | P | D | A | Call sites |
|---|---|---|---|---|
| `vcs.refreshStatus` | P1 | yes | yes | D `src/branch_picker.rs:87`, `src/git_controls.rs`; A `AC/git/GitOperations.kt:34` |
| `vcs.listRefs` | P1 | yes | yes | D `src/branch_picker.rs:34`; A `AC/git/GitReviewReader.kt:47` |
| `vcs.createRef`, `vcs.switchRef` | P1 | yes | domain-built | D `src/branch_picker.rs:55,64` |
| `vcs.pull` | P1 | yes | yes | D `src/git_operations.rs:245`; A `AC/git/GitOperations.kt:64` |
| `vcs.init` | P2 | yes | no | D `src/write.rs:410` |
| WS `git.runStackedAction` (`commit`,`push`,`create_pr`,`commit_push`,`commit_push_pr`, commit message ≤10 000 chars) | P1 | yes | yes | D `src/git_operations.rs:199-201`; A `AC/git/NativeGitStackedAction.kt:23`, `GitOperations.kt:51-61`. Stream of progress events, `Start::Events`. |
| `git.preparePullRequestThread` | P2 | yes | no | D `src/pull_request_actions.rs:38` |
| `pullRequests.list/detail/checks/activity/routing/routingIdentity/reviewerCandidates/labelCandidates/changeBreakdown/filterOptions` | P2 | yes | partial | D `src/connection_github.rs:16-38`, `:30`; `src/write.rs:360`; A reads via domain recipes `AC/pullrequests/PullRequestReader.kt:12-32` (list/detail/checks/activity; labelled tests-only for `list` here but reachable through recipe `method`) |
| `pullRequests.comment/submitReview/runAction/requestReviewers/setLabels` | P2 | yes | partial (`comment`, `submitReview`, `runAction`) | D `src/pull_request_actions.rs:11`; A `AC/pullrequests/PullRequestWrites.kt:20-22` |

## 8. Terminal, preview, devices, providers, ops

| Surface | P | D | A | Call sites |
|---|---|---|---|---|
| WS `terminal.attach` | P1 | yes | no (fixture only) | D `src/terminal.rs:61-68` (CLI TTY client launched by the "open terminal" action; `terminal::context/session/run_session` in lib) |
| `terminal.open`, `terminal.write`, `terminal.resize` | P1 | yes | yes (write/resize; open via git stacked scripts) | D `src/terminal.rs:79-80,88`, `src/git_operations.rs:255-259`; A `AC/terminal/TerminalBindings.kt:21-28`, `AC/git/GitOperations.kt:77` |
| WS `terminal.observe` | unused | no | no | wrapper only |
| `preview.list/open/navigate/resize/adjust/refresh/close/clearProfile/reportStatus` | P2 | yes | no | D `src/preview_actions.rs:66-137`, `src/preview_transport.rs:277`, `src/browser_profiles.rs:206` |
| WS `subscribePreviewEvents`, `subscribeDiscoveredLocalServers` / `preview.discoveredServers` | P2 | yes | no | D `src/preview_transport.rs:296`, `src/local_servers.rs:13-14` |
| `device.list/configure/testHost/open/close/detail/action` + WS `subscribeDeviceState` | P2 | yes | no | D `src/integration_devices.rs:174-179,415-457`, `src/device_screen.rs:88-113,277`; device-hub HTTP/WS `/api/device-hub/**` (screenshots, mjpeg, WS with `wsTicket`) `L/src/device_hub.rs`, D `src/device_screen.rs:172` |
| `device.easAccount` | P2, nightly | yes | no | D `src/nightly.rs:42` |
| `provider.auth.start/respond/cancel/logout`, `provider.install.start/cancel/remove`, WS `provider.auth.subscribe`, `provider.install.subscribe` | P2 | yes | yes (start, respond, cancel, logout, install start/cancel, + state reads) | D `src/provider_setup.rs:45-57`; A `AC/providers/ProviderAdministration.kt:50-91` |
| `scheduledTasks.list/upsert/setEnabled/delete/runNow` | P2 | yes | yes | D `src/scheduled_tasks.rs:64-68`; A `AA/ui/settings/maintenance/MaintenanceScreen.kt:72-85`, `AC/maintenance/MaintenanceController.kt:17` |
| `agentSessions.scan/import` | P2 | yes | yes | D `src/agent_sessions.rs:9,26`; A `MaintenanceController.kt:17,152` |
| `server.getHostResources`, `server.getProcessDiagnostics`, `server.getResourceTelemetryHistory`, `server.signalProcess`, `server.checkOtlpEndpoint` | P2 | yes | yes (processes, telemetry, signal) | D `src/connection_route.rs:40`, `src/process_diagnostics.rs:15-28`, `src/write.rs:407`; A `MaintenanceController.kt:17,50,154` |
| `server.runStorageCleanup`, WS `server.getStorageCleanupReport` (`diagnostics.storageCleanupReport` composed read) | P2 | yes | yes | D `src/storage_cleanup.rs:19-36`; A `MaintenanceScreen.kt:58`, `MaintenanceController.kt:16,139` |
| `mcpApps.callTool/readResource/updateModelContext` | P2 | yes | no | D `src/mcp_apps.rs:43` |
| `secrets.answerRequest` (`{threadId,turnItemId,answer:{type:"save",secret}\|{type:"decline"}}`) | P1 | yes | yes | D `src/write.rs:465-470`; A `AA/ui/thread/reader/SecretRequest.kt:47` |
| `guestShare.create` | P2, nightly | yes | no | D `src/nightly.rs:23` |

### Lib wrappers with **no** client call site (can be omitted from o3-server)
`provider.switch`, `thread.stop`, `thread.title.regeneration.complete`, `thread.created.record`, `notification.delivery.accept`, `terminal.observe`, `device` wrappers in Android, `preview.*` in Android, `projectClone.*`/`projects.createNew` in Android, `server.upsertKeybinding`/`removeKeybinding`/ACP registry/`checkOtlpEndpoint`/`getHostResources` in Android, `mcpApps.*` in Android, `guestShare.create` in Android, `orchestration.getThreadFamily`/`getWorktreeThreadIds`/`getOlderSettledThreads`/`getWorkflowScript`/`stopWorkflow`/`getFullThreadDiff`/`getThreadProjection`/`getTurnItem` in Android (the last via inspector recipe only).

---

## 9. Raw strings and paths outside the lib wrappers

Raw `rpc("…")` / tag literals outside `api::<mod>::<fn>`:

| Raw tag | Where | Why |
|---|---|---|
| `attachments.createUploadUrl` | `L/src/lib.rs:405` (inside the lib, the only raw `rpc`) | upload grant |
| any tag string | D `src/write.rs:8-10` (`write::rpc(e, tag, payload)`) | doc says "ot3 itself sends every RPC through `protocol::api`"; no live caller found |
| `protocol::api::call(conn, method, payload)` / `function(tag).call` | D `src/failure_context.rs:38`, `src/connection_github.rs:17-18`, `src/pull_request_actions.rs:11`, `src/mcp_apps.rs:43`, `src/nightly.rs:55`, `src/integration_devices.rs:349-351`, `src/git_operations.rs:201` | tag chosen from the registry at runtime (pull requests, mcp, nightly, device, git stacked) |
| Android `transport.call(fn, args)` with computed `fn` | `AC/providers/ProviderAdministration.kt:74` (provider actions), `AC/pullrequests/PullRequestReader.kt:17,32` & `PullRequestWrites.kt:22` (`recipe.method`), `AC/maintenance/MaintenanceController.kt:48,83,95` (page read + action fns), `AC/git/GitOperations.kt:63` (`steps[].tag`, SUPPORTED = `git.runStackedAction`, `vcs.pull`, `terminal.open`, `terminal.write` at `:77`), `AA/ui/thread/ThreadViewModel.kt:752` (tool inspector), `AA/ui/thread/reader/TranscriptConnection.kt:11-15` (only `secrets.answerRequest`) | method names come from lib domain recipes |
| Android literal `client.call("server.getConfig")` etc. | listed in §4 | straight calls |

Raw HTTP paths outside lib wrapper functions (desktop, via `Client::get` / `request_json`):
`/api/auth/session` (14 files), `/api/orchestration/shell` (12 files), `/api/projects` (3), `/.well-known/t3/environment` (5), `/api/auth/clients`, `/api/auth/clients/revoke`, `/api/auth/clients/revoke-others`, `/api/auth/pairing-links`, `/api/auth/pairing-links/revoke`, `/api/auth/pairing-token`, `/api/connect/link-state`, `/api/connect/preferences`, `/api/connect/reachability`, `/api/pull-requests/diff`, `/api/device-hub/**` (see lib). Android: `/api/auth/session` (`NativeT3Transport.kt:49`), `/api/orchestration/shell` (`:224`). Everything else is inside the lib (`/oauth/token`, `/api/orchestration/threads/*/bounded|history`, `/api/attachments/upload/*`, `/api/assets/*`, `/api/auth/websocket-ticket`, `/ws`).

Desktop also opens a **browser URL** `<origin>/#/<environmentId>/<threadId>` for "open in T3" (falls back from `t3code://threads/…` via `gtk-launch`): `D/src/main.rs:191-211`, `D/src/desktop_open.rs`. Needs the server to serve the T3 web app only if that fallback matters. Not P0.

---

## 10. Things the desktop does WITHOUT the server (o3-server must stay drop-in)

The local environment is **never** discovered over the network; it is read from the T3 profile on disk and the `t3` CLI. These are hard compatibility requirements for a drop-in.

**Profile directory.** `T3CODE_HOME` (env, trimmed, absolute) else `~/.t3` (`D/src/connections.rs:11-14`). Files under `<home>/userdata/`:

| File | Read/written by desktop | Fields / purpose | Evidence |
|---|---|---|---|
| `server-runtime.json` | read (poll + SHA hash for change detection) | `origin` (must be `http://127.0.0.1`/`localhost`/`[::1]`), `pid` (liveness via `kill(pid,0)`; also `/proc/<pid>/stat` zombie check), `port` (SSH helper). **o3-server must write it on start, delete on stop.** | D `src/connections.rs:158-163`, `src/backend_service.rs:23-29`, `src/connections_management.rs:71-72`; remote scripts `src/remote-bootstrap.sh:6-9`, `remote-restart.sh` |
| `environment-id` | read | primary environment uuid (plain text, trimmed). Must equal descriptor `environmentId`; remote scripts exit 7 on mismatch. | D `src/connections.rs:357`, `src/connection_route.rs:29`, `src/server_updates.rs` |
| `connection-catalog.json` | read + write (with `.ot3-connection-catalog.lock`, backs up ciphertext) | saved remote connections: `targets[]` (`BearerConnectionTarget`, `SshConnectionTarget`, `RelayConnectionTarget`), `profiles[]` (`httpBaseUrl`, ssh `target`), `credentials[]` (`BearerConnectionCredential.token`), `disabledEnvironmentIds`. **v11 is sealed with a secret in the login keyring** (`secret-tool lookup application t3code`, `gdbus … SearchItems {'application':'t3code'}`) → encrypted with AES-CBC/PBKDF2 (crate deps `aes`, `cbc`, `pbkdf2`, `sha1`). o3-server does not need to write it, but if it replaces T3 on the same profile it must not break the format. | D `src/connections.rs:31-62`, `src/connections_management.rs:6,245`, `L/src/connection_catalog.rs` |
| `desktop-settings.json` | read + write | `localEnvironmentEnabled`, `tailscaleServePort`, … | D `src/connections.rs:363`, `src/connections_management.rs:61,309` |
| `client-settings.json` | read + write (with lock `.ot3-client-settings.lock`) | `mathRenderingEnabled`, `planModeEnabled`, `loadBalancingEnabled`, client prefs | D `src/connection_preferences.rs:9,64`, `D/src/lib.rs:112` |
| `clerk-tokens.json` | read (hash only) | T3 Connect cloud login state | D `src/connections.rs:351` |
| `secrets/server-signing-key.bin` | mtime only | local token older than this key is discarded and re-issued | D `src/connections.rs:167` |
| `ssh-launch/*/port`, `ssh-launch/*/pid` | read by remote bootstrap script | T3 Desktop's SSH launcher state | `D/src/remote-bootstrap.sh:13-19`, `remote-restart.sh` |
| `ot3-update.lock` | written (remote update script) | update serialization | `D/src/remote-update.sh:18` |
| (additional dir) `userdata/attachments/` | **not read by clients** | server-side attachment store; clients only go through the upload/asset URLs | no match in D/L |

**Local bearer token.** `$XDG_DATA_HOME/local.write.token` (default `~/.local/share/local.write.token`, mode 0600, parent 0700). Valid if fresh (>1 h to expiry), newer than `server-signing-key.bin`, and its payload carries scopes `filesystem:read`, `settings:write`, `terminal:read`, `terminal:operate`. Otherwise the desktop **shells out**:

```
timeout 12s ~/.t3/bin/t3 (else `t3` on PATH) auth session issue --base-dir <T3CODE_HOME>
  --scope orchestration:read --scope orchestration:operate --scope filesystem:read --scope settings:write
  --scope terminal:read --scope terminal:operate --ttl 30d --label omarchy-t3-code --token-only
```
stdout = token (≥16 chars, no whitespace). Remote bootstrap script adds `--subject omarchy-t3-code[:<16hex>]` (falls back without `--subject` for older T3) and more scopes (`providers:manage`, … truncated in `remote-bootstrap.sh:50`). (`D/src/connections.rs:165-186`, `D/src/remote-bootstrap.sh`.)

**`t3` CLI subcommands the desktop invokes** (binary `~/.t3/bin/t3`, then `~/.local/bin/t3`, then PATH). o3-server must ship a binary answering these (or the desktop is patched):

| Command | Purpose | Evidence |
|---|---|---|
| `t3 auth session issue --base-dir <h> [--subject S] --scope … --ttl 30d --label L --token-only` | mint bearer | `D/src/connections.rs:171`, `remote-bootstrap.sh` |
| `t3 service install --base-dir <h>` | install + start the systemd user unit | `D/src/backend_service.rs:149`, scripts |
| `t3 __ssh-helper runtime-port <runtime.json>` → stdout `"<something> <port>"` (exit 0) | port from runtime file | `D/src/backend_service.rs:168`, scripts |
| `t3 __ssh-helper wait-ready <port> <timeoutMs> <intervalMs>` | HTTP readiness | `D/src/backend_service.rs:183-190`, scripts |
| `t3 update --yes --base-dir <h> <exact-version>` | self-update | `D/src/remote-update.sh:31`, `D/src/server_updates.rs` |
| `t3 status|login|link|unlink|logout --base-dir <h>` (T3 Connect) | cloud connect | `D/src/connection_cloud.rs:42-50` |
| `t3 serve` / `__service-launcher` | **not invoked with args by the desktop**; only recognized in `/proc/<pid>/cmdline` by `remote-restart.sh` (`grep -E '/t3 (serve|__service-launcher)'`) when killing a stale server, and by `backend_service::evidence` (cmdline contains `/apps/server/*.mjs|*.js` + `T3CODE_HOME=<home>` in environ) to classify `headless-manual`. So an o3-server binary named `t3` with argv0/`serve` subcommand matches the kill logic. | `D/src/remote-restart.sh`, `D/src/backend_service.rs:77-102` |

**Headless unit management (desktop is the controller).** Unit `~/.config/systemd/user/t3code.service`; ownership is proven by a line `Environment=T3CODE_HOME=<home>` (quoted or not). The desktop runs `timeout 30 systemctl --user {start|stop|restart} t3code.service`, classifies the running server (cgroup contains `t3code.service` → `headless`; ancestor in cgroup `app-com.t3tools.T3Code*.scope` or exe `t3code|t3-code|T3 Code` without `ELECTRON_RUN_AS_NODE=1` → `gui`; env `T3CODE_HOME` + cmdline `/apps/server/*.mjs` + same uid → `headless-manual`) and never stops a GUI-owned server (`backend_owned_by_gui`). Manual stop = `SIGTERM`, waits ≤10 s. Start waits ≤15 s for `runtime-port` + `wait-ready 15000 1000`. Update: `systemctl --user kill --signal=KILL; reset-failed; rm runtime; restart`. (`D/src/backend_service.rs`, `D/src/remote-restart.sh`.) Env vars the clients respect: `T3CODE_HOME`, `T3CODE_TAILSCALE_SERVE_PORT` (`D/src/connection_network.rs:46`), `OMARCHY_T3_RELEASE_INDEX`, `GH_TOKEN|GITHUB_TOKEN`.

**SSH transport to remote T3 hosts** (hosts we also might run o3-server on): `ssh` with ControlMaster in `$XDG_RUNTIME_DIR/ot3-ssh/%C`, pipes `remote-bootstrap.sh` via `sh -s -- <environmentId> <clientId>`; output = `server-runtime.json` text + `\nT3_AUTH\n` + token; then `ssh -N -L` tunnel from `127.0.0.1:<port>` and `fetchDescriptor` identity check. Remote host therefore needs: `~/.t3/userdata/environment-id`, `server-runtime.json`, `t3` CLI at the paths above, optionally the systemd unit. (`D/src/connections.rs:188-320`.)

**Other external things the desktop touches (not server work, listed for completeness):** `secret-tool`/`gdbus org.freedesktop.secrets` (decrypting the T3 catalog), `tailscale serve status --json`, NetworkManager/logind D-Bus (reconnect on resume, `D/src/reconnect.rs`), GitHub releases API (`https://api.github.com/repos/pingdotgg/t3code/releases`, server update advisor), `gtk-launch com.t3tools.T3Code|t3code t3code://threads/<env>/<thread>`, `xdg-open`, dev-vault git/`~/Obsidian/Dev`, OmaBridge, host-sync of agent config files (`~/.claude/…`, `~/.codex/config.toml`). None need server cooperation.

**Android** has no filesystem coupling to a T3 profile: it stores environments + bearer tokens itself (Room + `CredentialStore`/`TokenStore`), pairs through `/oauth/token`, and optionally imports a bridge catalog (`AC/bridge/*`). Everything it needs is HTTP/WS.

---

## 11. Proposed P0 implementation order for o3-server

Goal of each step: a client capability becomes testable end to end before moving on.

1. **Process shell + profile.** Listen on 127.0.0.1:<port>; honour `T3CODE_HOME`; create/read `userdata/environment-id`; write `userdata/server-runtime.json {origin,pid,port}` on start, remove on stop; signing key at `userdata/secrets/server-signing-key.bin`. Ship a `t3`-compatible CLI (or `o3` aliasing `t3`) with `auth session issue … --token-only`, `__ssh-helper runtime-port|wait-ready`, `service install`, `serve`. (§10)
2. **Token format + auth.** Issue/verify `payload.signature` tokens (claims `scopes[]`, `exp` ms); bearer middleware on HTTP and WS upgrade; scope check per RPC; `EnvironmentAuthorizationError{requiredScope}` failure shape. `GET /api/auth/session`. (§0)
3. **`GET /.well-known/t3/environment`** (`environmentId`, `label`, `serverVersion`, `platform.os`, `orchestrationProtocolVersion`) + **`POST /oauth/token`** (token-exchange of a pairing secret) + `POST /api/auth/pairing-token` (so Android can be paired; plus `t3 auth pairing create` equivalent for headless). (§1)
4. **Effect-RPC WebSocket** `/ws?orchestrationProtocol=2`: Request/Exit(Success|Failure)/Chunk/Ping/Pong/Interrupt framing, one-socket-per-unary-call tolerance, stream lifecycle. (§0)
5. **`server.getConfig`** (providers[], settings, environment, capability flags all false) and **`server.getSettings`/`updateSettings`**. Both clients refuse to render the composer/new-thread without it. (§4)
6. **Event store + projection**: threads, projects, messages, runs, turnItems, with a monotonic `sequence`. Then `GET /api/orchestration/shell` (+`protocol` header handling), `orchestration.subscribeShell` (snapshot, then events; resume by `afterSequence` or fresh snapshot). (§2)
7. **`GET /api/orchestration/threads/{id}/bounded`** and `orchestration.subscribeThread` (snapshot + events; ignore optional `accept*` flags on first pass). (§2)
8. **`orchestration.dispatchCommand` with `commandId` dedup** for: `message.dispatch`, `run.interrupt`, `thread.create`, `runtime-request.respond`, `thread.user-input.dismiss`; **`orchestration.launchThread`**. (§3)
9. **Provider runtime**: actually run Codex/Claude sessions, emit run/turn-item events into the stream (this is the product, not a client call, but P0 for chat).
10. **Next tier (P1)**, in this order: `attachments.createUploadUrl` + `POST /api/attachments/upload/{id}` + `assets.createUrl` + `GET /api/assets/*`; thread lifecycle commands (`archive/pin/settle/snooze/metadata.update/fork/visit/mark-unread/queue*`); `GET …/history`; `server.subscribeServerConfig`; `projects.readFile/searchEntries/mutate`, `filesystem.browse`; `orchestration.getTurnDiff`/`getFullThreadDiff`/`getTurnItem`/`searchThreads`/`subscribeCommandOutput`; `vcs.*`, `git.runStackedAction`, `review.getDiffPreview`; `terminal.*`; `server.refreshProviders`; `secrets.answerRequest`; `server.reportClientActivity`.
11. **P2 (defer)**: provider auth/install, scheduledTasks, agentSessions, diagnostics/storage cleanup, preview/devices/mcpApps, keybindings, ACP registry, pull requests, guest share, auth client management, T3 Connect, nightly/capability-gated reads.

Open questions for the author (not resolved by the research): (a) should o3-server also embed the `t3` CLI surface or should the desktop be changed to call `o3`; (b) whether the T3 web app must be served at `/` for the "open in browser" fallback; (c) encrypted `connection-catalog.json` is T3 Desktop's, so confirm o3-server never needs to read it; (d) exact JSON shapes should be taken from `L/tests/fixtures/t3-contract.json` and the T3 checkout via `L/tools/extract-t3-contract`, which this research did not re-run.
