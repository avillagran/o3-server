# Compatibilidad o3-server ↔ T3

Contrato: `t3code@3f9448747`. Generado por `tools/gen-compat`; editar `docs/compat-status.json`.

Leyenda: ✅ hecho · 🟡 parcial · ⬜ pendiente · 🚫 fuera de alcance (ningún cliente lo usa)

**Resumen:** 2 ✅ · 6 🟡 · 112 ⬜ · 83 🚫 de 203 RPC

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
| ⬜ | `subscribeServerConfig` | P1 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |
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
| ⬜ | `assets.createUrl` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `assets.persistChatAttachments` | unused | unary | `orchestration:operate` | No client call site in inventory. |

## attachments

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `attachments.createUploadUrl` | P1 | unary | `orchestration:operate` | Client usage inventory; deferred. |
| ⬜ | `attachments.delete` | P2 | unary | `orchestration:operate` | Deferred; client-usage.md inventory. |

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
| ⬜ | `filesystem.browse` | P1 | unary | `filesystem:read` | Deferred; client-usage.md inventory. |
| ⬜ | `filesystem.getMetadata` | P2 | unary | `filesystem:read` | Deferred; client-usage.md inventory. |

## git

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `git.preparePullRequestThread` | P2 | unary | `source-control:write` | Deferred; client-usage.md inventory. |
| 🚫 | `git.resolvePullRequest` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `git.runStackedAction` | P1 | stream | `source-control:write` | Deferred; client-usage.md inventory. |

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
| ⬜ | `orchestration.getArchivedShellSnapshot` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `orchestration.getFullThreadDiff` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `orchestration.getOlderSettledThreads` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `orchestration.getThreadFamily` | unused | unary | `orchestration:read` | No client call site in inventory. |
| ⬜ | `orchestration.getThreadProjection` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `orchestration.getTurnDiff` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `orchestration.getTurnItem` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `orchestration.getWorkflowScript` | P2 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `orchestration.getWorktreeThreadIds` | unused | unary | `orchestration:read` | No client call site in inventory. |
| 🟡 | `orchestration.launchThread` | P0 | unary | `orchestration:operate` | Connection test: idempotent empty root-workspace creation; worktrees, reuse and atomic multi-stage launch remain. |
| ⬜ | `orchestration.searchThread` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `orchestration.searchThreadStream` | unused | stream | `orchestration:read` | No client call site in inventory. |
| ⬜ | `orchestration.searchThreads` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `orchestration.stopWorkflow` | P2 | unary | `orchestration:operate` | Deferred; client-usage.md inventory. |
| 🚫 | `orchestration.subscribeArchivedShell` | unused | stream | `orchestration:read` | No client call site in inventory. |
| ⬜ | `orchestration.subscribeCommandOutput` | P1 | stream | `orchestration:read` | Deferred; client-usage.md inventory. |
| ✅ | `orchestration.subscribeShell` | P0 | stream | `orchestration:read` | Connection tests: SQLite snapshot, replay afterSequence, synchronized completion marker; shared project/thread sequence. |
| 🟡 | `orchestration.subscribeThread` | P0 | stream | `orchestration:read` | Connection tests: bounded snapshot, replay and marker; always bounded even without opt-in, advanced provider projections absent. |

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
| 🟡 | `projects.mutate` | P1 | unary | `orchestration:operate` | Connection test: project create/dedup/shared log; update/delete and repository identity not yet covered. |
| ⬜ | `projects.readFile` | P1 | unary | `filesystem:read` | Deferred; client-usage.md inventory. |
| 🚫 | `projects.searchContents` | unused | unary | `filesystem:read` | No client call site in inventory. |
| ⬜ | `projects.searchEntries` | P1 | unary | `filesystem:read` | Deferred; client-usage.md inventory. |
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
| ⬜ | `review.getDiffFileContents` | P1 | unary | `filesystem:read` | Deferred; client-usage.md inventory. |
| ⬜ | `review.getDiffPreview` | P1 | unary | `filesystem:read` | Deferred; client-usage.md inventory. |

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
| 🟡 | `server.getConfig` | P0 | unary | `orchestration:read` | Real Connection test: descriptor/settings and empty providers; defaults are a subset, provider status catalog absent. |
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
| 🟡 | `server.updateSettings` | P1 | unary | `settings:write` | Connection test: patch/persistence/redaction; providerInstanceMutation and full upstream validation/merge semantics remain. |
| ⬜ | `server.upsertKeybinding` | P2 | unary | `settings:write` | Deferred; client-usage.md inventory. |

## shell

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `shell.openInEditor` | P2 | unary | `orchestration:operate` | Deferred; client-usage.md inventory. |

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
| ⬜ | `vcs.createRef` | P1 | unary | `source-control:write` | Deferred; client-usage.md inventory. |
| 🚫 | `vcs.createWorktree` | unused | unary | `source-control:write` | No client call site in inventory. |
| ⬜ | `vcs.init` | P2 | unary | `source-control:write` | Deferred; client-usage.md inventory. |
| ⬜ | `vcs.listRefs` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| ⬜ | `vcs.pull` | P1 | unary | `source-control:write` | Deferred; client-usage.md inventory. |
| ⬜ | `vcs.refreshStatus` | P1 | unary | `orchestration:read` | Deferred; client-usage.md inventory. |
| 🚫 | `vcs.removeWorktree` | unused | unary | `source-control:write` | No client call site in inventory. |
| ⬜ | `vcs.switchRef` | P1 | unary | `source-control:write` | Deferred; client-usage.md inventory. |

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
| 🟡 | `message.dispatch` | P0 | Real Connection scripted provider test; production delivery/recovery not complete. |
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
| 🟡 | `run.interrupt` | P0 | Real Connection scripted provider test; production delivery/recovery not complete. |
| 🟡 | `runtime-request.respond` | P0 | Real Connection scripted provider test; production delivery/recovery not complete. |
| ⬜ | `subagent.stop` | P2 | Not implemented. |
| ⬜ | `thread.active.reorder` | P1 | Not implemented. |
| ✅ | `thread.archive` | P1 | Real Connection lifecycle/idempotency test. |
| ⬜ | `thread.auto-settle` | unused | Not implemented. |
| ⬜ | `thread.auto-settle.set` | P1 | Not implemented. |
| ✅ | `thread.create` | P0 | Real Connection lifecycle/idempotency test. |
| 🚫 | `thread.created.record` | unused | No client call site in inventory. |
| ✅ | `thread.delete` | P1 | Real Connection lifecycle/idempotency test. |
| ⬜ | `thread.fork` | P1 | Not implemented. |
| ⬜ | `thread.guest-message.dismiss` | P2 | Not implemented. |
| ⬜ | `thread.guest-share.close` | P2 | Not implemented. |
| ⬜ | `thread.interaction-mode.set` | P1 | Not implemented. |
| ⬜ | `thread.mark-unread` | P1 | Not implemented. |
| 🚫 | `thread.merge_back` | unused | No client call site in inventory. |
| ✅ | `thread.metadata.update` | P1 | Real Connection lifecycle/idempotency test. |
| ⬜ | `thread.model-selection.set` | P1 | Not implemented. |
| ✅ | `thread.pin` | P1 | Real Connection lifecycle/idempotency test. |
| ⬜ | `thread.pin.reorder` | P1 | Not implemented. |
| ⬜ | `thread.pull-request-link.sync` | P2 | Not implemented. |
| ⬜ | `thread.pull-request.link` | P2 | Not implemented. |
| ⬜ | `thread.pull-request.sync` | P2 | Client usage inventory; deferred. |
| ⬜ | `thread.pull-request.unlink` | P2 | Client usage inventory; deferred. |
| ⬜ | `thread.pull-request.watch` | P2 | Client usage inventory; deferred. |
| ⬜ | `thread.runtime-mode.set` | P1 | Not implemented. |
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
