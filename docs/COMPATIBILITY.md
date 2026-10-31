# Compatibilidad o3-server ↔ T3

Contrato: `t3code@3f9448747`. Generado por `tools/gen-compat`; editar `docs/compat-status.json`.

Leyenda: ✅ hecho · 🟡 parcial · ⬜ pendiente · 🚫 fuera de alcance (ningún cliente lo usa)

**Resumen:** 0 ✅ · 1 🟡 · 202 ⬜ · 0 🚫 de 203 RPC

## (root)

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `subscribeAuthAccess` | ? | stream | `access:read` |  |
| ⬜ | `subscribeBackgroundPolicy` | ? | stream | `orchestration:read` |  |
| ⬜ | `subscribeDeviceState` | ? | stream | `orchestration:read` |  |
| ⬜ | `subscribeDiscoveredLocalServers` | ? | stream | `orchestration:read` |  |
| ⬜ | `subscribePreviewEvents` | ? | stream | `orchestration:read` |  |
| ⬜ | `subscribeProjectClones` | ? | stream | `orchestration:read` |  |
| ⬜ | `subscribeResourceTelemetry` | ? | stream | `diagnostics:read` |  |
| ⬜ | `subscribeServerConfig` | ? | stream | `orchestration:read` |  |
| ⬜ | `subscribeServerLifecycle` | ? | stream | `orchestration:read` |  |
| ⬜ | `subscribeTerminalEvents` | ? | stream | `terminal:read` |  |
| ⬜ | `subscribeTerminalMetadata` | ? | stream | `terminal:read` |  |
| ⬜ | `subscribeVcsStatus` | ? | stream | `orchestration:read` |  |
| ⬜ | `subscribeWorktreeSetup` | ? | stream | `orchestration:read` |  |

## agentSessions

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `agentSessions.import` | ? | unary | `orchestration:operate` |  |
| ⬜ | `agentSessions.scan` | ? | unary | `orchestration:read` |  |

## assets

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `assets.createUrl` | ? | unary | `orchestration:read` |  |
| ⬜ | `assets.persistChatAttachments` | ? | unary | `orchestration:operate` |  |

## attachments

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `attachments.createUploadUrl` | ? | unary | `orchestration:operate` |  |
| ⬜ | `attachments.delete` | ? | unary | `orchestration:operate` |  |

## cloud

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `cloud.getRelayClientStatus` | ? | unary | `relay:read` |  |
| ⬜ | `cloud.installRelayClient` | ? | stream | `relay:write` |  |

## device

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `device.action` | ? | unary | `orchestration:operate` |  |
| ⬜ | `device.close` | ? | unary | `orchestration:operate` |  |
| ⬜ | `device.configure` | ? | unary | `settings:write` |  |
| ⬜ | `device.detail` | ? | unary | `orchestration:read` |  |
| ⬜ | `device.easAccount` | ? | unary | `settings:write` |  |
| ⬜ | `device.list` | ? | unary | `orchestration:read` |  |
| ⬜ | `device.open` | ? | unary | `orchestration:operate` |  |
| ⬜ | `device.shutdown` | ? | unary | `orchestration:operate` |  |
| ⬜ | `device.testHost` | ? | unary | `settings:write` |  |

## filesystem

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `filesystem.browse` | ? | unary | `filesystem:read` |  |
| ⬜ | `filesystem.getMetadata` | ? | unary | `filesystem:read` |  |

## git

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `git.preparePullRequestThread` | ? | unary | `source-control:write` |  |
| ⬜ | `git.resolvePullRequest` | ? | unary | `orchestration:read` |  |
| ⬜ | `git.runStackedAction` | ? | stream | `source-control:write` |  |

## guestShare

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `guestShare.create` | ? | unary | `access:write` |  |

## mcpApps

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `mcpApps.callTool` | ? | unary | `orchestration:operate` |  |
| ⬜ | `mcpApps.readResource` | ? | unary | `orchestration:read` |  |
| ⬜ | `mcpApps.toolInfo` | ? | unary | `orchestration:read` |  |
| ⬜ | `mcpApps.updateModelContext` | ? | unary | `orchestration:operate` |  |

## orchestration

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `orchestration.dispatchCommand` | ? | unary | `orchestration:operate` |  |
| ⬜ | `orchestration.getArchivedShellSnapshot` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.getFullThreadDiff` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.getOlderSettledThreads` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.getThreadFamily` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.getThreadProjection` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.getTurnDiff` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.getTurnItem` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.getWorkflowScript` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.getWorktreeThreadIds` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.launchThread` | ? | unary | `orchestration:operate` |  |
| ⬜ | `orchestration.searchThread` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.searchThreadStream` | ? | stream | `orchestration:read` |  |
| ⬜ | `orchestration.searchThreads` | ? | unary | `orchestration:read` |  |
| ⬜ | `orchestration.stopWorkflow` | ? | unary | `orchestration:operate` |  |
| ⬜ | `orchestration.subscribeArchivedShell` | ? | stream | `orchestration:read` |  |
| ⬜ | `orchestration.subscribeCommandOutput` | ? | stream | `orchestration:read` |  |
| 🟡 | `orchestration.subscribeShell` | P0 | stream | `orchestration:read` | Stream real con snapshot vacío; falta proyección de hilos/proyectos y eventos |
| ⬜ | `orchestration.subscribeThread` | ? | stream | `orchestration:read` |  |

## preview

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `preview.adjust` | ? | unary | `preview:operate` |  |
| ⬜ | `preview.clearProfile` | ? | unary | `preview:operate` |  |
| ⬜ | `preview.close` | ? | unary | `preview:operate` |  |
| ⬜ | `preview.list` | ? | unary | `orchestration:read` |  |
| ⬜ | `preview.navigate` | ? | unary | `preview:operate` |  |
| ⬜ | `preview.open` | ? | unary | `preview:operate` |  |
| ⬜ | `preview.refresh` | ? | unary | `preview:operate` |  |
| ⬜ | `preview.reportProfiles` | ? | unary | `preview:operate` |  |
| ⬜ | `preview.reportStatus` | ? | unary | `preview:operate` |  |
| ⬜ | `preview.resize` | ? | unary | `preview:operate` |  |

## projectClone

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `projectClone.cancel` | ? | unary | `source-control:write` |  |
| ⬜ | `projectClone.retry` | ? | unary | `source-control:write` |  |
| ⬜ | `projectClone.start` | ? | unary | `source-control:write` |  |

## projects

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `projects.createNew` | ? | unary | `orchestration:operate` |  |
| ⬜ | `projects.ensureScratch` | ? | unary | `orchestration:operate` |  |
| ⬜ | `projects.listEntries` | ? | unary | `filesystem:read` |  |
| ⬜ | `projects.listIgnoredFiles` | ? | unary | `filesystem:read` |  |
| ⬜ | `projects.mutate` | ? | unary | `orchestration:operate` |  |
| ⬜ | `projects.readFile` | ? | unary | `filesystem:read` |  |
| ⬜ | `projects.searchContents` | ? | unary | `filesystem:read` |  |
| ⬜ | `projects.searchEntries` | ? | unary | `filesystem:read` |  |
| ⬜ | `projects.writeFile` | ? | unary | `filesystem:write` |  |

## provider

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `provider.auth.cancel` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.auth.complete` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.auth.logout` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.auth.respond` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.auth.start` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.auth.subscribe` | ? | stream | `providers:manage` |  |
| ⬜ | `provider.chatgpt.handoff.subscribe` | ? | stream | `providers:manage` |  |
| ⬜ | `provider.chatgpt.import-profile` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.chatgpt.reconnect-profile` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.codex.auth-callback.subscribe` | ? | stream | `providers:manage` |  |
| ⬜ | `provider.consumeResetCredit` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.install.cancel` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.install.remove` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.install.start` | ? | unary | `providers:manage` |  |
| ⬜ | `provider.install.subscribe` | ? | stream | `orchestration:read` |  |
| ⬜ | `provider.uploadFeedback` | ? | unary | `orchestration:operate` |  |

## pullRequests

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `pullRequests.activity` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.changeBreakdown` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.checks` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.comment` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.detail` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.diffFileContents` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.filesViewed` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.filterOptions` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.generateReviewGuide` | ? | unary | `orchestration:operate` |  |
| ⬜ | `pullRequests.invalidate` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.labelCandidates` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.linkedThreads` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.list` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.listStats` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.preview` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.replyToThread` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.reportState` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.requestReviewers` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.reviewGuide` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.reviewerCandidates` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.routing` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.routingIdentity` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.runAction` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.setFilesViewed` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.setLabels` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.setReaction` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.setThreadResolution` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.stack` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.submitReview` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.subscribeRefreshes` | ? | stream | `orchestration:read` |  |
| ⬜ | `pullRequests.summary` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.threadComments` | ? | unary | `orchestration:read` |  |
| ⬜ | `pullRequests.update` | ? | unary | `source-control:write` |  |
| ⬜ | `pullRequests.updateComment` | ? | unary | `source-control:write` |  |

## review

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `review.getDiffFileContents` | ? | unary | `filesystem:read` |  |
| ⬜ | `review.getDiffPreview` | ? | unary | `filesystem:read` |  |

## scheduledTasks

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `scheduledTasks.delete` | ? | unary | `orchestration:operate` |  |
| ⬜ | `scheduledTasks.getWebhookDelivery` | ? | unary | `orchestration:operate` |  |
| ⬜ | `scheduledTasks.list` | ? | unary | `orchestration:read` |  |
| ⬜ | `scheduledTasks.listWebhookDeliveries` | ? | unary | `orchestration:operate` |  |
| ⬜ | `scheduledTasks.rotateWebhookToken` | ? | unary | `orchestration:operate` |  |
| ⬜ | `scheduledTasks.runNow` | ? | unary | `orchestration:operate` |  |
| ⬜ | `scheduledTasks.setEnabled` | ? | unary | `orchestration:operate` |  |
| ⬜ | `scheduledTasks.subscribe` | ? | stream | `orchestration:read` |  |
| ⬜ | `scheduledTasks.upsert` | ? | unary | `orchestration:operate` |  |

## secrets

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `secrets.answerRequest` | ? | unary | `orchestration:operate` |  |

## server

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `server.acceptAcpRegistryUrlAuth` | ? | unary | `providers:manage` |  |
| ⬜ | `server.checkOtlpEndpoint` | ? | unary | `settings:write` |  |
| ⬜ | `server.commitDesktopUpdate` | ? | unary | `environment:maintain` |  |
| ⬜ | `server.deleteAcpRegistrySession` | ? | unary | `orchestration:operate` |  |
| ⬜ | `server.disableAcpRegistryProvider` | ? | unary | `providers:manage` |  |
| ⬜ | `server.discoverSourceControl` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.getBackgroundPolicy` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.getConfig` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.getDatabaseStorage` | ? | stream | `orchestration:read` |  |
| ⬜ | `server.getHostResources` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.getProcessDiagnostics` | ? | unary | `diagnostics:read` |  |
| ⬜ | `server.getProcessResourceHistory` | ? | unary | `diagnostics:read` |  |
| ⬜ | `server.getResourceTelemetryHistory` | ? | unary | `diagnostics:read` |  |
| ⬜ | `server.getSettings` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.getStorageCleanupReport` | ? | stream | `orchestration:read` |  |
| ⬜ | `server.getTraceDiagnostics` | ? | unary | `diagnostics:read` |  |
| ⬜ | `server.getUsageSummary` | ? | unary | `diagnostics:read` |  |
| ⬜ | `server.importAcpRegistrySession` | ? | unary | `orchestration:operate` |  |
| ⬜ | `server.listAcpRegistryProviders` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.listAcpRegistrySessions` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.logoutAcpRegistry` | ? | unary | `providers:manage` |  |
| ⬜ | `server.prepareAcpRegistryAgent` | ? | unary | `providers:manage` |  |
| ⬜ | `server.probe` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.refreshProviders` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.refreshUsageRates` | ? | unary | `diagnostics:read` |  |
| ⬜ | `server.removeKeybinding` | ? | unary | `settings:write` |  |
| ⬜ | `server.reportClientActivity` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.reportHostPowerState` | ? | unary | `environment:maintain` |  |
| ⬜ | `server.retryResourceTelemetry` | ? | unary | `diagnostics:read` |  |
| ⬜ | `server.runStorageCleanup` | ? | unary | `settings:write` |  |
| ⬜ | `server.searchAcpRegistry` | ? | unary | `orchestration:read` |  |
| ⬜ | `server.setAcpRegistryProvider` | ? | unary | `providers:manage` |  |
| ⬜ | `server.signalProcess` | ? | unary | `environment:maintain` |  |
| ⬜ | `server.uninstallAcpRegistryManagedBinary` | ? | unary | `providers:manage` |  |
| ⬜ | `server.updateProvider` | ? | unary | `providers:manage` |  |
| ⬜ | `server.updateServer` | ? | unary | `environment:maintain` |  |
| ⬜ | `server.updateServerWithProgress` | ? | stream | `environment:maintain` |  |
| ⬜ | `server.updateSettings` | ? | unary | `settings:write` |  |
| ⬜ | `server.upsertKeybinding` | ? | unary | `settings:write` |  |

## shell

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `shell.openInEditor` | ? | unary | `orchestration:operate` |  |

## sourceControl

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `sourceControl.cloneRepository` | ? | unary | `source-control:write` |  |
| ⬜ | `sourceControl.lookupRepository` | ? | unary | `orchestration:read` |  |
| ⬜ | `sourceControl.publishRepository` | ? | unary | `source-control:write` |  |

## terminal

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `terminal.attach` | ? | stream | `terminal:operate` |  |
| ⬜ | `terminal.clear` | ? | unary | `terminal:operate` |  |
| ⬜ | `terminal.close` | ? | unary | `terminal:operate` |  |
| ⬜ | `terminal.observe` | ? | stream | `terminal:read` |  |
| ⬜ | `terminal.open` | ? | unary | `terminal:operate` |  |
| ⬜ | `terminal.resize` | ? | unary | `terminal:operate` |  |
| ⬜ | `terminal.restart` | ? | unary | `terminal:operate` |  |
| ⬜ | `terminal.write` | ? | unary | `terminal:operate` |  |

## vcs

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `vcs.createRef` | ? | unary | `source-control:write` |  |
| ⬜ | `vcs.createWorktree` | ? | unary | `source-control:write` |  |
| ⬜ | `vcs.init` | ? | unary | `source-control:write` |  |
| ⬜ | `vcs.listRefs` | ? | unary | `orchestration:read` |  |
| ⬜ | `vcs.pull` | ? | unary | `source-control:write` |  |
| ⬜ | `vcs.refreshStatus` | ? | unary | `orchestration:read` |  |
| ⬜ | `vcs.removeWorktree` | ? | unary | `source-control:write` |  |
| ⬜ | `vcs.switchRef` | ? | unary | `source-control:write` |  |

## worktreeSetup

| Estado | RPC | Prioridad | Tipo | Scope | Nota |
|---|---|---|---|---|---|
| ⬜ | `worktreeSetup.cancel` | ? | unary | `orchestration:operate` |  |
