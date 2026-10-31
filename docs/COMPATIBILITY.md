# Compatibilidad o3-server ↔ T3

Contrato: `t3code@3f9448747`. Generado por `tools/gen-compat`; editar `docs/compat-status.json`.

Leyenda: ✅ hecho · 🟡 parcial · ⬜ pendiente · 🚫 fuera de alcance (ningún cliente lo usa)

**Resumen:** 16 ✅ · 16 🟡 · 88 ⬜ · 83 🚫 de 203 RPC

## (root)

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| 🚫 | `subscribeAuthAccess` | unused | stream | `access:read` | No client call site in inventory. |
| 🚫 | `subscribeBackgroundPolicy` | unused | stream | `orchestration:read` | No client call site in inventory. |
| ⬜ | `subscribeDeviceState` | P2 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `subscribeDiscoveredLocalServers` | P2 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `subscribePreviewEvents` | P2 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `subscribeProjectClones` | unused | stream | `orchestration:read` | No client call site in inventory. |
| 🚫 | `subscribeResourceTelemetry` | unused | stream | `diagnostics:read` | No client call site in inventory. |
| 🟡 | `subscribeServerConfig` | P1 | stream | `orchestration:read` | Connection test: initial config snapshot; changes emit replacement snapshots rather than typed incremental config events. Also accepts server.subscribeServerConfig alias used by client inventory. |
| 🚫 | `subscribeServerLifecycle` | unused | stream | `orchestration:read` | No client call site in inventory. |
| 🚫 | `subscribeTerminalEvents` | unused | stream | `terminal:read` | No client call site in inventory. |
| 🚫 | `subscribeTerminalMetadata` | unused | stream | `terminal:read` | No client call site in inventory. |
| 🚫 | `subscribeVcsStatus` | unused | stream | `orchestration:read` | No client call site in inventory. |
| ⬜ | `subscribeWorktreeSetup` | P2 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |

## agentSessions

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `agentSessions.import` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |
| ⬜ | `agentSessions.scan` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |

## assets

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| 🟡 | `assets.createUrl` | P1 | unary | `orchestration:read` | Tested: attachment, workspace-file (incl. HTML dependency grants), media-file (identity-pinned), draft-workspace-file, project-favicon (+missing sentinel). GET /api/assets/* streams, supports Range/HEAD, rejects traversal/forged/expired tokens. tool-output-image is implemented but untested; native-app-icon and github-media are refused; mcp-app HTML injection not done. |
| 🚫 | `assets.persistChatAttachments` | unused | unary | `orchestration:operate` | No client call site in inventory. |

## attachments

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ✅ | `attachments.createUploadUrl` | P1 | unary | `orchestration:operate` | Real-client test (files_assets): HMAC-signed 10 min grant, type/size limits (10 MiB image, 50 MiB file), pending-<uuid>[-ext] ids. Token layout matches T3. |
| ✅ | `attachments.delete` | P2 | unary | `orchestration:operate` | Real-client test: removes pending uploads (and .txt sidecar), idempotent, ignores non-pending or malformed ids. Upload route: streamed to a .part file, exact size, 204, 409 on conflicting duplicate. |

## cloud

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| 🚫 | `cloud.getRelayClientStatus` | unused | unary | `relay:read` | No client call site in inventory. |
| 🚫 | `cloud.installRelayClient` | unused | stream | `relay:write` | No client call site in inventory. |

## device

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `device.action` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |
| ⬜ | `device.close` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |
| ⬜ | `device.configure` | P2 | unary | `settings:write` | Client usage inventory; deferred. |
| ⬜ | `device.detail` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| ⬜ | `device.easAccount` | P2 | unary | `settings:write` | Deferred; client-usage.md inventory. |
| ⬜ | `device.list` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `device.open` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |
| 🚫 | `device.shutdown` | unused | unary | `orchestration:operate` | No client call site in inventory. |
| ⬜ | `device.testHost` | P2 | unary | `settings:write` | Client usage inventory; deferred. |

## filesystem

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ✅ | `filesystem.browse` | P1 | unary | `filesystem:read` | Real-client test: prefix filter, hidden folders, explicit-relative paths need cwd, Windows paths refused, unreadable dir error. |
| ✅ | `filesystem.getMetadata` | P2 | unary | `filesystem:read` | Real-client test: file/directory/null, header MIME sniff for extensionless files, batch limit of 64. |

## git

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `git.preparePullRequestThread` | P2 | unary | `source-control:write` | Deferred; client-usage.md inventory. |
| 🚫 | `git.resolvePullRequest` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🟡 | `git.runStackedAction` | P1 | stream | `source-control:write` | Real-client stream test: commit, commit_push, push, commit_push_pr (with gh), featureBranch, filePaths subset, up-to-date and no-change results, action_failed with the failing phase, request validation, path confinement. Not done: hook_* events, generated commit messages, cancellation of a running step (an Interrupt stops sending events but the step finishes), PR creation without gh. |

## guestShare

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `guestShare.create` | P2 | unary | `access:write` | Deferred; client-usage.md inventory. |

## mcpApps

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `mcpApps.callTool` | P2 | unary | `orchestration:operate` | Deferred; client-usage.md inventory. |
| ⬜ | `mcpApps.readResource` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| 🚫 | `mcpApps.toolInfo` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `mcpApps.updateModelContext` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |

## orchestration

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| 🟡 | `orchestration.dispatchCommand` | P0 | unary | `orchestration:operate` | Connection tests: transactional command receipts, lifecycle, scripted send/approval/completion/interrupt/dismiss; queues and production execution absent. |
| 🟡 | `orchestration.getArchivedShellSnapshot` | P2 | unary | `orchestration:read` | Real-client test: archived threads and projects with the shared sequence. Built from the in-memory shell assembly (10,000 thread cap), same limit as the live shell. |
| 🟡 | `orchestration.getFullThreadDiff` | P1 | unary | `orchestration:read` | Same as getTurnDiff: tested against seeded checkpoints; capture is not implemented. |
| ⬜ | `orchestration.getOlderSettledThreads` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `orchestration.getThreadFamily` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ✅ | `orchestration.getThreadProjection` | P2 | unary | `orchestration:read` | Real-client test: bounded projection (up to 100 rows per kind) and an error for unknown threads. |
| 🟡 | `orchestration.getTurnDiff` | P1 | unary | `orchestration:read` | Tested against seeded checkpoint records and real git refs (ranges, whitespace flag, unavailable turns, invalid refs and unknown workspaces refused). o3-server does not capture checkpoints yet, so no thread produces them. |
| ✅ | `orchestration.getTurnItem` | P1 | unary | `orchestration:read` | Real-client test: item by id for its own thread, null otherwise. The revision argument is ignored. |
| ⬜ | `orchestration.getWorkflowScript` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `orchestration.getWorktreeThreadIds` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🟡 | `orchestration.launchThread` | P0 | unary | `orchestration:operate` | Connection: idempotent root launch. Core tests: new git worktree, existing worktree, thread reuse and receipts. Atomic create+initial dispatch, workflow cleanup and reuse metadata semantics remain. |
| ⬜ | `orchestration.searchThread` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `orchestration.searchThreadStream` | unused | stream | `orchestration:read` | No client call site in inventory. |
| 🟡 | `orchestration.searchThreads` | P1 | unary | `orchestration:read` | Real-client test: SQLite FTS5 index kept by message writes and rebuilt when missing; best message per thread with hits/quality/snippet, prefix match, project filter, titlesOnly, archived/deleted excluded, FTS syntax inert. Not done: threads/olderSettled fields, environmentLabel. |
| ⬜ | `orchestration.stopWorkflow` | P2 | unary | `orchestration:operate` | Deferred; client-usage.md inventory. |
| 🚫 | `orchestration.subscribeArchivedShell` | unused | stream | `orchestration:read` | No client call site in inventory. |
| ⬜ | `orchestration.subscribeCommandOutput` | P1 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🟡 | `orchestration.subscribeShell` | P0 | stream | `orchestration:read` | Real Connection snapshot/replay tests. SQLite streaming removes the 10,000 cap (10,002-project streaming test); omitIdleSubagents/omitOlderSettled and family shapes remain. |
| 🟡 | `orchestration.subscribeThread` | P0 | stream | `orchestration:read` | Real Connection tests: streamed full opt-out (140 messages), bounded ten-turn window, compact IDs/body dedupe/local turn-item omission, replay/markers. Generic/activity/oversized and targeted-history policies are not fully conformant. |

## preview

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `preview.adjust` | P2 | unary | `preview:operate` | Client usage inventory; deferred. |
| ⬜ | `preview.clearProfile` | P2 | unary | `preview:operate` | Client usage inventory; deferred. |
| ⬜ | `preview.close` | P2 | unary | `preview:operate` | Client usage inventory; deferred. |
| ⬜ | `preview.list` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `preview.navigate` | P2 | unary | `preview:operate` | Client usage inventory; deferred. |
| ⬜ | `preview.open` | P2 | unary | `preview:operate` | Client usage inventory; deferred. |
| ⬜ | `preview.refresh` | P2 | unary | `preview:operate` | Client usage inventory; deferred. |
| 🚫 | `preview.reportProfiles` | unused | unary | `preview:operate` | No client call site in inventory. |
| ⬜ | `preview.reportStatus` | P2 | unary | `preview:operate` | Client usage inventory; deferred. |
| ⬜ | `preview.resize` | P2 | unary | `preview:operate` | Client usage inventory; deferred. |

## projectClone

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| 🚫 | `projectClone.cancel` | unused | unary | `source-control:write` | No client call site in inventory. |
| 🚫 | `projectClone.retry` | unused | unary | `source-control:write` | No client call site in inventory. |
| ⬜ | `projectClone.start` | P2 | unary | `source-control:write` | Deferred; client-usage.md inventory. |

## projects

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `projects.createNew` | P2 | unary | `orchestration:operate` | Deferred; client-usage.md inventory. |
| 🚫 | `projects.ensureScratch` | unused | unary | `orchestration:operate` | No client call site in inventory. |
| 🚫 | `projects.listEntries` | unused | unary | `filesystem:read` | No client call site in inventory. |
| ⬜ | `projects.listIgnoredFiles` | P2 | unary | `filesystem:read` | Deferred; client-usage.md inventory. |
| 🟡 | `projects.mutate` | P1 | unary | `orchestration:operate` | Tested over RPC and POST /api/projects/mutate: create/update/delete, idempotent replay, workspace-root validation (exists, createWorkspaceRootIfMissing, no duplicate roots), delete needs force when threads exist, shell events project.updated/removed on the shared sequence, scope checks. Not done: repositoryIdentity, force-delete does not cascade to threads. |
| ✅ | `projects.readFile` | P1 | unary | `filesystem:read` | Real-client test: relative paths rooted lexically and by realpath (symlink escapes refused), 1 MiB cap with truncated flag, binary refused, allowMissing, absolute host paths read in place as T3 does, filesystem:read scope. |
| 🚫 | `projects.searchContents` | unused | unary | `filesystem:read` | No client call site in inventory. |
| ✅ | `projects.searchEntries` | P1 | unary | `filesystem:read` | Real-client test: git ignore rules, kind/imageOnly filters, limit clamp, truncated flag, missing root error. Ranking is o3's own (basename, then path, then subsequence), not T3's fff finder; index capped at 25,000 entries. |
| 🚫 | `projects.writeFile` | unused | unary | `filesystem:write` | No client call site in inventory. |

## provider

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `provider.auth.cancel` | P2 | unary | `providers:manage` | Client usage inventory; deferred. |
| 🚫 | `provider.auth.complete` | unused | unary | `providers:manage` | No client call site in inventory. |
| ⬜ | `provider.auth.logout` | P2 | unary | `providers:manage` | Client usage inventory; deferred. |
| ⬜ | `provider.auth.respond` | P2 | unary | `providers:manage` | Client usage inventory; deferred. |
| ⬜ | `provider.auth.start` | P2 | unary | `providers:manage` | Deferred; client-usage.md inventory. |
| ⬜ | `provider.auth.subscribe` | P2 | stream | `providers:manage` | Deferred; client-usage.md inventory. |
| 🚫 | `provider.chatgpt.handoff.subscribe` | unused | stream | `providers:manage` | No client call site in inventory. |
| 🚫 | `provider.chatgpt.import-profile` | unused | unary | `providers:manage` | No client call site in inventory. |
| 🚫 | `provider.chatgpt.reconnect-profile` | unused | unary | `providers:manage` | No client call site in inventory. |
| 🚫 | `provider.codex.auth-callback.subscribe` | unused | stream | `providers:manage` | No client call site in inventory. |
| 🚫 | `provider.consumeResetCredit` | unused | unary | `providers:manage` | No client call site in inventory. |
| ⬜ | `provider.install.cancel` | P2 | unary | `providers:manage` | Client usage inventory; deferred. |
| ⬜ | `provider.install.remove` | P2 | unary | `providers:manage` | Client usage inventory; deferred. |
| ⬜ | `provider.install.start` | P2 | unary | `providers:manage` | Deferred; client-usage.md inventory. |
| ⬜ | `provider.install.subscribe` | P2 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `provider.uploadFeedback` | unused | unary | `orchestration:operate` | No client call site in inventory. |

## pullRequests

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `pullRequests.activity` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| ⬜ | `pullRequests.changeBreakdown` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| ⬜ | `pullRequests.checks` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| ⬜ | `pullRequests.comment` | P2 | unary | `source-control:write` | Deferred; client-usage.md inventory. |
| ⬜ | `pullRequests.detail` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| 🚫 | `pullRequests.diffFileContents` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🚫 | `pullRequests.filesViewed` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `pullRequests.filterOptions` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| 🚫 | `pullRequests.generateReviewGuide` | unused | unary | `orchestration:operate` | No client call site in inventory. |
| 🚫 | `pullRequests.invalidate` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `pullRequests.labelCandidates` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| 🚫 | `pullRequests.linkedThreads` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `pullRequests.list` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `pullRequests.listStats` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🚫 | `pullRequests.preview` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🚫 | `pullRequests.replyToThread` | unused | unary | `source-control:write` | No client call site in inventory. |
| 🚫 | `pullRequests.reportState` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `pullRequests.requestReviewers` | P2 | unary | `source-control:write` | Client usage inventory; deferred. |
| 🚫 | `pullRequests.reviewGuide` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `pullRequests.reviewerCandidates` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| ⬜ | `pullRequests.routing` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| ⬜ | `pullRequests.routingIdentity` | P2 | unary | `orchestration:read` | Client usage inventory; deferred. |
| ⬜ | `pullRequests.runAction` | P2 | unary | `source-control:write` | Client usage inventory; deferred. |
| 🚫 | `pullRequests.setFilesViewed` | unused | unary | `source-control:write` | No client call site in inventory. |
| ⬜ | `pullRequests.setLabels` | P2 | unary | `source-control:write` | Client usage inventory; deferred. |
| 🚫 | `pullRequests.setReaction` | unused | unary | `source-control:write` | No client call site in inventory. |
| 🚫 | `pullRequests.setThreadResolution` | unused | unary | `source-control:write` | No client call site in inventory. |
| 🚫 | `pullRequests.stack` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `pullRequests.submitReview` | P2 | unary | `source-control:write` | Client usage inventory; deferred. |
| 🚫 | `pullRequests.subscribeRefreshes` | unused | stream | `orchestration:read` | No client call site in inventory. |
| 🚫 | `pullRequests.summary` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🚫 | `pullRequests.threadComments` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🚫 | `pullRequests.update` | unused | unary | `source-control:write` | No client call site in inventory. |
| 🚫 | `pullRequests.updateComment` | unused | unary | `source-control:write` | No client call site in inventory. |

## review

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ✅ | `review.getDiffFileContents` | P1 | unary | `filesystem:read` | Real-client test: old/new contents for working-tree and branch-range, new/deleted, escapes, symlinks and binary files refused, known-workspace check. |
| ✅ | `review.getDiffPreview` | P1 | unary | `filesystem:read` | Real-client test: working-tree and branch-range sources with untracked files (throwaway index), single-file and ignoreWhitespace requests, 120 kB truncation, non-repo yields no sources, cwd must be a known workspace. |

## scheduledTasks

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `scheduledTasks.delete` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |
| 🚫 | `scheduledTasks.getWebhookDelivery` | unused | unary | `orchestration:operate` | No client call site in inventory. |
| ⬜ | `scheduledTasks.list` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `scheduledTasks.listWebhookDeliveries` | unused | unary | `orchestration:operate` | No client call site in inventory. |
| 🚫 | `scheduledTasks.rotateWebhookToken` | unused | unary | `orchestration:operate` | No client call site in inventory. |
| ⬜ | `scheduledTasks.runNow` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |
| ⬜ | `scheduledTasks.setEnabled` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |
| 🚫 | `scheduledTasks.subscribe` | unused | stream | `orchestration:read` | No client call site in inventory. |
| ⬜ | `scheduledTasks.upsert` | P2 | unary | `orchestration:operate` | Client usage inventory; deferred. |

## secrets

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `secrets.answerRequest` | P1 | unary | `orchestration:operate` | Client usage inventory; deferred. |

## server

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| 🚫 | `server.acceptAcpRegistryUrlAuth` | unused | unary | `providers:manage` | No client call site in inventory. |
| ⬜ | `server.checkOtlpEndpoint` | P2 | unary | `settings:write` | Deferred; client-usage.md inventory. |
| 🚫 | `server.commitDesktopUpdate` | unused | unary | `environment:maintain` | No client call site in inventory. |
| 🚫 | `server.deleteAcpRegistrySession` | unused | unary | `orchestration:operate` | No client call site in inventory. |
| 🚫 | `server.disableAcpRegistryProvider` | unused | unary | `providers:manage` | No client call site in inventory. |
| ⬜ | `server.discoverSourceControl` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `server.getBackgroundPolicy` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🟡 | `server.getConfig` | P0 | unary | `orchestration:read` | Real Connection test: Codex app-server probe advertises actual auth/status and native model catalog. Other provider catalogs and setup flows remain partial. |
| 🚫 | `server.getDatabaseStorage` | unused | stream | `orchestration:read` | No client call site in inventory. |
| ⬜ | `server.getHostResources` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `server.getProcessDiagnostics` | P2 | unary | `diagnostics:read` | Deferred; client-usage.md inventory. |
| 🚫 | `server.getProcessResourceHistory` | unused | unary | `diagnostics:read` | No client call site in inventory. |
| ⬜ | `server.getResourceTelemetryHistory` | P2 | unary | `diagnostics:read` | Deferred; client-usage.md inventory. |
| ✅ | `server.getSettings` | P1 | unary | `orchestration:read` | Connection test: persisted settings, unknown fields and secret sentinel round-trip. |
| ⬜ | `server.getStorageCleanupReport` | P2 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `server.getTraceDiagnostics` | unused | unary | `diagnostics:read` | No client call site in inventory. |
| ⬜ | `server.getUsageSummary` | P2 | unary | `diagnostics:read` | Deferred; client-usage.md inventory. |
| 🚫 | `server.importAcpRegistrySession` | unused | unary | `orchestration:operate` | No client call site in inventory. |
| 🚫 | `server.listAcpRegistryProviders` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🚫 | `server.listAcpRegistrySessions` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🚫 | `server.logoutAcpRegistry` | unused | unary | `providers:manage` | No client call site in inventory. |
| ⬜ | `server.prepareAcpRegistryAgent` | P2 | unary | `providers:manage` | Deferred; client-usage.md inventory. |
| 🚫 | `server.probe` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `server.refreshProviders` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `server.refreshUsageRates` | P2 | unary | `diagnostics:read` | Deferred; client-usage.md inventory. |
| ⬜ | `server.removeKeybinding` | P2 | unary | `settings:write` | Deferred; client-usage.md inventory. |
| ⬜ | `server.reportClientActivity` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `server.reportHostPowerState` | unused | unary | `environment:maintain` | No client call site in inventory. |
| 🚫 | `server.retryResourceTelemetry` | unused | unary | `diagnostics:read` | No client call site in inventory. |
| ⬜ | `server.runStorageCleanup` | P2 | unary | `settings:write` | Deferred; client-usage.md inventory. |
| ⬜ | `server.searchAcpRegistry` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `server.setAcpRegistryProvider` | unused | unary | `providers:manage` | No client call site in inventory. |
| ⬜ | `server.signalProcess` | P2 | unary | `environment:maintain` | Deferred; client-usage.md inventory. |
| 🚫 | `server.uninstallAcpRegistryManagedBinary` | unused | unary | `providers:manage` | No client call site in inventory. |
| ⬜ | `server.updateProvider` | P2 | unary | `providers:manage` | Deferred; client-usage.md inventory. |
| 🚫 | `server.updateServer` | unused | unary | `environment:maintain` | No client call site in inventory. |
| 🚫 | `server.updateServerWithProgress` | unused | stream | `environment:maintain` | No client call site in inventory. |
| 🟡 | `server.updateSettings` | P1 | unary | `settings:write` | Connection tests: provider instance create/upsert/remove, redacted environment preservation, entry replacement/null removal, directory history, primitive type checks. Full upstream normalization/model preferences/background policy remain. |
| ⬜ | `server.upsertKeybinding` | P2 | unary | `settings:write` | Deferred; client-usage.md inventory. |

## shell

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| 🟡 | `shell.openInEditor` | P2 | unary | `orchestration:operate` | Real-client test with a fake editor on PATH: argv launch without a shell, unknown editor and option-like cwd refused, missing editor fails. Opens the folder only: goto/line-column styles and file-manager reveal are not implemented. |

## sourceControl

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| 🚫 | `sourceControl.cloneRepository` | unused | unary | `source-control:write` | No client call site in inventory. |
| 🚫 | `sourceControl.lookupRepository` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🚫 | `sourceControl.publishRepository` | unused | unary | `source-control:write` | No client call site in inventory. |

## terminal

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `terminal.attach` | P1 | stream | `terminal:operate` | Deferred; client-usage.md inventory. |
| 🚫 | `terminal.clear` | unused | unary | `terminal:operate` | No client call site in inventory. |
| 🚫 | `terminal.close` | unused | unary | `terminal:operate` | No client call site in inventory. |
| 🚫 | `terminal.observe` | unused | stream | `terminal:read` | No client call site in inventory. |
| ⬜ | `terminal.open` | P1 | unary | `terminal:operate` | Deferred; client-usage.md inventory. |
| ⬜ | `terminal.resize` | P1 | unary | `terminal:operate` | Deferred; client-usage.md inventory. |
| 🚫 | `terminal.restart` | unused | unary | `terminal:operate` | No client call site in inventory. |
| ⬜ | `terminal.write` | P1 | unary | `terminal:operate` | Deferred; client-usage.md inventory. |

## vcs

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ✅ | `vcs.createRef` | P1 | unary | `source-control:write` | Real-client test: create, switchRef, duplicate refused with branch_already_exists, hostile names refused, concurrent creates serialized per repository. |
| 🚫 | `vcs.createWorktree` | unused | unary | `source-control:write` | No client call site in inventory. |
| ✅ | `vcs.init` | P2 | unary | `source-control:write` | Real-client test: git init (also on an existing repo); jj is refused as unsupported; missing cwd is an error. |
| ✅ | `vcs.listRefs` | P1 | unary | `orchestration:read` | Real-client test: current/default first, local and remote refs, worktreePath, pagination (cursor/limit/nextCursor), query, refKind, matching-remote dedupe. The refresh flag is ignored (no cache). |
| ✅ | `vcs.pull` | P1 | unary | `source-control:write` | Real-client test: fast-forward pulled, up-to-date skipped, no upstream and diverged history are errors; --ff-only, never merges. |
| 🟡 | `vcs.refreshStatus` | P1 | unary | `orchestration:read` | Real-client test: non-repo, clean, dirty (modified/staged/untracked line counts), upstream ahead/behind, branchChanges vs default branch, aheadOfDefaultCount. pr is always null and sourceControlProvider is absent (no host adapter); localOnly ignored. |
| 🚫 | `vcs.removeWorktree` | unused | unary | `source-control:write` | No client call site in inventory. |
| ✅ | `vcs.switchRef` | P1 | unary | `source-control:write` | Real-client test: local switch, remote-only ref becomes a tracking branch, unknown ref refused. |

## worktreeSetup

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `worktreeSetup.cancel` | P2 | unary | `orchestration:operate` | Deferred; client-usage.md inventory. |

## Commands

| Estado | Command | Prioridad | Nota |
|---|---|---|---|
| ⬜ | `checkpoint.rollback` | P2 | Not implemented. |
| 🚫 | `delegated_task.completion-delivery.acknowledge` | unused | No client call site in inventory. |
| 🚫 | `delegated_task.completion-delivery.dispose` | unused | No client call site in inventory. |
| 🚫 | `delegated_task.request` | unused | No client call site in inventory. |
| 🚫 | `delegated_task.wake-policy` | unused | No client call site in inventory. |
| 🟡 | `message.dispatch` | P0 | Real Connection Codex stdio-peer test: turn start and steer_active; persisted native resume binding. Authenticated live inference, attachments, context, queues and crash recovery remain. |
| 🚫 | `notification.delivery.accept` | unused | No client call site in inventory. |
| 🚫 | `prepared-run.fail` | unused | No client call site in inventory. |
| 🚫 | `prepared-run.progress` | unused | No client call site in inventory. |
| 🚫 | `prepared-run.release` | unused | No client call site in inventory. |
| 🚫 | `prepared-run.retry` | unused | No client call site in inventory. |
| 🚫 | `provider-session.detach` | unused | No client call site in inventory. |
| 🚫 | `provider.switch` | unused | No client call site in inventory. |
| ⬜ | `queue.resume` | P1 | Client queue builders; deferred. |
| ⬜ | `queued-message.promote-to-steer` | P1 | Not implemented. |
| ⬜ | `queued-run.cancel` | P1 | Not implemented. |
| ⬜ | `queued-run.edit` | P1 | Client queue builders; deferred. |
| ⬜ | `queued-run.reorder` | P1 | Not implemented. |
| 🚫 | `queued-run.start` | unused | No client call site in inventory. |
| 🟡 | `run.interrupt` | P0 | Real Connection Codex stdio-peer test: live approval nodes/items, numeric native ID reply and idempotent receipt; answer/elicitation shapes unit-tested. Crash recovery remains. |
| 🟡 | `runtime-request.respond` | P0 | Observed can_use_tool allow/deny mapping; fixture approval projection and Connection responses. Live test approves Write; persistent approvals and structured questions remain. |
| ⬜ | `subagent.stop` | P2 | Not implemented. |
| ⬜ | `thread.active.reorder` | P1 | Not implemented. |
| ✅ | `thread.archive` | P1 | Real Connection lifecycle/idempotency test. |
| 🚫 | `thread.auto-settle` | unused | No client call site in inventory. |
| ⬜ | `thread.auto-settle.set` | P1 | Not implemented. |
| ✅ | `thread.create` | P0 | Real Connection lifecycle/idempotency test. |
| 🚫 | `thread.created.record` | unused | No client call site in inventory. |
| ✅ | `thread.delete` | P1 | Real Connection lifecycle/idempotency test. |
| ⬜ | `thread.fork` | P1 | Not implemented. |
| ⬜ | `thread.guest-message.dismiss` | P2 | Not implemented. |
| ⬜ | `thread.guest-share.close` | P2 | Not implemented. |
| 🟡 | `thread.interaction-mode.set` | P1 | Idle default/plan updates persisted and Connection-tested; native mode applied on the next turn. Active-run mutation rejected. |
| ⬜ | `thread.mark-unread` | P1 | Not implemented. |
| 🚫 | `thread.merge_back` | unused | No client call site in inventory. |
| ✅ | `thread.metadata.update` | P1 | Real Connection lifecycle/idempotency test. |
| 🟡 | `thread.model-selection.set` | P1 | Idle updates persisted and Connection-tested; native set_model applied on the next turn. Active-run mutation rejected. |
| ✅ | `thread.pin` | P1 | Real Connection lifecycle/idempotency test. |
| ⬜ | `thread.pin.reorder` | P1 | Not implemented. |
| ⬜ | `thread.pull-request-link.sync` | P2 | Not implemented. |
| ⬜ | `thread.pull-request.link` | P2 | Not implemented. |
| ⬜ | `thread.pull-request.sync` | P2 | Client usage inventory; deferred. |
| ⬜ | `thread.pull-request.unlink` | P2 | Client usage inventory; deferred. |
| ⬜ | `thread.pull-request.watch` | P2 | Client usage inventory; deferred. |
| 🟡 | `thread.runtime-mode.set` | P1 | Idle updates persisted and Connection-tested; native permission mode applied on the next turn. Active-run mutation rejected. |
| ✅ | `thread.settle` | P1 | Real Connection lifecycle/idempotency test. |
| ⬜ | `thread.snooze` | P1 | Not implemented. |
| 🚫 | `thread.stop` | unused | No client call site in inventory. |
| 🚫 | `thread.title.regeneration.complete` | unused | No client call site in inventory. |
| ✅ | `thread.unarchive` | P1 | Real Connection lifecycle/idempotency test. |
| ✅ | `thread.unpin` | P1 | Real Connection lifecycle/idempotency test. |
| ✅ | `thread.unsettle` | P1 | Real Connection lifecycle/idempotency test. |
| ⬜ | `thread.unsnooze` | P1 | Not implemented. |
| 🟡 | `thread.user-input.dismiss` | P1 | Real Connection scripted provider test; production delivery/recovery not complete. |
| ✅ | `thread.visit` | P1 | Real Connection lifecycle/idempotency test. |
